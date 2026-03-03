//! Adaptive provider router with metrics-driven selection.
//!
//! Replaces static priority failover with a scoring system that tracks
//! per-provider latency (EMA + p95), error rates, and circuit breaker state.
//! Supports probe/canary requests to keep metrics fresh for non-primary providers.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use async_trait::async_trait;
use crew_core::Message;
use eyre::Result;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::config::ChatConfig;
use crate::provider::LlmProvider;
use crate::types::{ChatResponse, ChatStream, ToolSpec};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Tuning knobs for the adaptive router.
#[derive(Debug, Clone)]
pub struct AdaptiveConfig {
    /// EMA smoothing factor (0..1). Higher = more responsive to recent latency.
    pub ema_alpha: f64,
    /// Consecutive failures before circuit breaker opens.
    pub failure_threshold: u32,
    /// Latency (ms) above which a soft penalty is applied.
    pub latency_threshold_ms: u64,
    /// Error rate (0..1) above which provider is deprioritized.
    pub error_rate_threshold: f64,
    /// Probability (0..1) of probing a non-primary provider.
    pub probe_probability: f64,
    /// Minimum seconds between probes to the same provider.
    pub probe_interval_secs: u64,
    /// Scoring weights (should sum to ~1.0).
    pub weight_latency: f64,
    pub weight_error_rate: f64,
    pub weight_priority: f64,
}

impl Default for AdaptiveConfig {
    fn default() -> Self {
        Self {
            ema_alpha: 0.3,
            failure_threshold: 3,
            latency_threshold_ms: 30_000,
            error_rate_threshold: 0.3,
            probe_probability: 0.1,
            probe_interval_secs: 60,
            weight_latency: 0.4,
            weight_error_rate: 0.4,
            weight_priority: 0.2,
        }
    }
}

// ---------------------------------------------------------------------------
// Per-provider metrics
// ---------------------------------------------------------------------------

const LATENCY_BUFFER_SIZE: usize = 64;

/// Circular buffer for computing p95 latency.
struct LatencySamples {
    buf: [u64; LATENCY_BUFFER_SIZE],
    len: usize,
    pos: usize,
}

impl LatencySamples {
    fn new() -> Self {
        Self {
            buf: [0; LATENCY_BUFFER_SIZE],
            len: 0,
            pos: 0,
        }
    }

    fn push(&mut self, us: u64) {
        self.buf[self.pos] = us;
        self.pos = (self.pos + 1) % LATENCY_BUFFER_SIZE;
        if self.len < LATENCY_BUFFER_SIZE {
            self.len += 1;
        }
    }

    fn p95(&self) -> u64 {
        if self.len == 0 {
            return 0;
        }
        let mut sorted: Vec<u64> = self.buf[..self.len].to_vec();
        sorted.sort_unstable();
        let idx = ((self.len as f64) * 0.95).ceil() as usize;
        sorted[idx.min(self.len) - 1]
    }
}

/// Metrics for a single provider slot.
struct ProviderMetrics {
    /// Exponential moving average of latency (microseconds).
    latency_ema_us: AtomicU64,
    /// p95 latency (microseconds), updated on each sample.
    p95_latency_us: AtomicU64,
    /// Total successful requests (monotonic).
    success_count: AtomicU32,
    /// Total failed requests (monotonic).
    failure_count: AtomicU32,
    /// Consecutive failures (resets on success). Circuit breaker trigger.
    consecutive_failures: AtomicU32,
    /// Epoch micros of last successful request.
    last_success_us: AtomicU64,
    /// Epoch micros of last request (success or failure).
    last_request_us: AtomicU64,
    /// Total requests counter for periodic logging.
    total_requests: AtomicU32,
    /// Circular buffer for p95 computation.
    latency_samples: Mutex<LatencySamples>,
}

impl ProviderMetrics {
    fn new() -> Self {
        Self {
            latency_ema_us: AtomicU64::new(0),
            p95_latency_us: AtomicU64::new(0),
            success_count: AtomicU32::new(0),
            failure_count: AtomicU32::new(0),
            consecutive_failures: AtomicU32::new(0),
            last_success_us: AtomicU64::new(0),
            last_request_us: AtomicU64::new(0),
            total_requests: AtomicU32::new(0),
            latency_samples: Mutex::new(LatencySamples::new()),
        }
    }

    fn record_success_with_alpha(&self, latency_us: u64, alpha: f64) {
        let now_us = now_epoch_us();

        let prev = self.latency_ema_us.load(Ordering::Relaxed);
        let new_ema = if prev == 0 {
            latency_us
        } else {
            ((alpha * latency_us as f64) + ((1.0 - alpha) * prev as f64)) as u64
        };
        self.latency_ema_us.store(new_ema, Ordering::Relaxed);

        if let Ok(mut samples) = self.latency_samples.lock() {
            samples.push(latency_us);
            self.p95_latency_us.store(samples.p95(), Ordering::Relaxed);
        }

        self.success_count.fetch_add(1, Ordering::Relaxed);
        self.consecutive_failures.store(0, Ordering::Relaxed);
        self.last_success_us.store(now_us, Ordering::Relaxed);
        self.last_request_us.store(now_us, Ordering::Relaxed);
        self.total_requests.fetch_add(1, Ordering::Relaxed);
    }

    fn record_failure(&self) {
        let now_us = now_epoch_us();
        self.failure_count.fetch_add(1, Ordering::Relaxed);
        self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
        self.last_request_us.store(now_us, Ordering::Relaxed);
        self.total_requests.fetch_add(1, Ordering::Relaxed);
    }

    fn error_rate(&self) -> f64 {
        let s = self.success_count.load(Ordering::Relaxed);
        let f = self.failure_count.load(Ordering::Relaxed);
        let total = s + f;
        if total == 0 {
            0.0
        } else {
            f as f64 / total as f64
        }
    }

    fn is_circuit_open(&self, threshold: u32) -> bool {
        self.consecutive_failures.load(Ordering::Relaxed) >= threshold
    }

    fn is_stale(&self, probe_interval_secs: u64) -> bool {
        let last = self.last_request_us.load(Ordering::Relaxed);
        if last == 0 {
            return true; // Never used
        }
        let elapsed_us = now_epoch_us().saturating_sub(last);
        elapsed_us > probe_interval_secs * 1_000_000
    }

    fn snapshot(&self) -> MetricsSnapshot {
        let s = self.success_count.load(Ordering::Relaxed);
        let f = self.failure_count.load(Ordering::Relaxed);
        MetricsSnapshot {
            latency_ema_ms: self.latency_ema_us.load(Ordering::Relaxed) as f64 / 1000.0,
            p95_latency_ms: self.p95_latency_us.load(Ordering::Relaxed) as f64 / 1000.0,
            success_count: s,
            failure_count: f,
            consecutive_failures: self.consecutive_failures.load(Ordering::Relaxed),
            error_rate: if s + f == 0 {
                0.0
            } else {
                f as f64 / (s + f) as f64
            },
        }
    }
}

/// Public snapshot of provider metrics for observability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub latency_ema_ms: f64,
    pub p95_latency_ms: f64,
    pub success_count: u32,
    pub failure_count: u32,
    pub consecutive_failures: u32,
    pub error_rate: f64,
}

/// Adaptive routing policy parameters for observability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedPolicy {
    pub ema_alpha: f64,
    pub failure_threshold: u32,
    pub latency_threshold_ms: u64,
    pub error_rate_threshold: f64,
    pub probe_probability: f64,
    pub probe_interval_secs: u64,
    pub weight_latency: f64,
    pub weight_error_rate: f64,
    pub weight_priority: f64,
}

/// Shared metrics file format for inter-process export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedMetrics {
    pub updated_at: String,
    pub policy: SharedPolicy,
    pub providers: Vec<SharedProviderMetrics>,
}

/// Per-provider metrics entry in the shared file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedProviderMetrics {
    pub provider: String,
    pub model: String,
    pub score: f64,
    #[serde(flatten)]
    pub metrics: MetricsSnapshot,
}

// ---------------------------------------------------------------------------
// Adaptive Router
// ---------------------------------------------------------------------------

/// A provider slot in the adaptive router.
struct AdaptiveSlot {
    provider: std::sync::Arc<dyn LlmProvider>,
    metrics: ProviderMetrics,
    /// Config-order priority (0 = primary, 1 = first fallback, etc.).
    priority: usize,
}

/// Adaptive provider router with metrics-driven selection.
///
/// Drop-in replacement for `ProviderChain`. Tracks latency and error rates
/// per provider, scores them dynamically, and routes to the best performer.
/// Probes stale providers to keep metrics fresh.
pub struct AdaptiveRouter {
    slots: Vec<AdaptiveSlot>,
    config: AdaptiveConfig,
    /// RNG state for probe selection (simple xorshift).
    rng_state: AtomicU64,
}

impl AdaptiveRouter {
    /// Create a new adaptive router from providers (in priority order).
    ///
    /// Panics if `providers` is empty.
    pub fn new(providers: Vec<std::sync::Arc<dyn LlmProvider>>, config: AdaptiveConfig) -> Self {
        assert!(
            !providers.is_empty(),
            "AdaptiveRouter requires at least one provider"
        );
        let slots = providers
            .into_iter()
            .enumerate()
            .map(|(i, p)| AdaptiveSlot {
                provider: p,
                metrics: ProviderMetrics::new(),
                priority: i,
            })
            .collect();
        Self {
            slots,
            config,
            rng_state: AtomicU64::new(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64,
            ),
        }
    }

    /// Get metrics snapshots for all providers (for observability / dashboard).
    pub fn metrics_snapshots(&self) -> Vec<(&str, &str, MetricsSnapshot)> {
        self.slots
            .iter()
            .map(|s| {
                (
                    s.provider.provider_name(),
                    s.provider.model_id(),
                    s.metrics.snapshot(),
                )
            })
            .collect()
    }

    /// Export metrics in the shared file format (sorted by score, lowest first).
    pub fn export_shared_metrics(&self) -> SharedMetrics {
        let mut providers: Vec<SharedProviderMetrics> = self
            .slots
            .iter()
            .map(|s| SharedProviderMetrics {
                provider: s.provider.provider_name().to_string(),
                model: s.provider.model_id().to_string(),
                score: self.score(s),
                metrics: s.metrics.snapshot(),
            })
            .collect();
        providers.sort_by(|a, b| {
            a.score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        SharedMetrics {
            updated_at: chrono::Utc::now().to_rfc3339(),
            policy: SharedPolicy {
                ema_alpha: self.config.ema_alpha,
                failure_threshold: self.config.failure_threshold,
                latency_threshold_ms: self.config.latency_threshold_ms,
                error_rate_threshold: self.config.error_rate_threshold,
                probe_probability: self.config.probe_probability,
                probe_interval_secs: self.config.probe_interval_secs,
                weight_latency: self.config.weight_latency,
                weight_error_rate: self.config.weight_error_rate,
                weight_priority: self.config.weight_priority,
            },
            providers,
        }
    }

    /// Score a provider. Lower is better.
    fn score(&self, slot: &AdaptiveSlot) -> f64 {
        let total = slot.metrics.success_count.load(Ordering::Relaxed)
            + slot.metrics.failure_count.load(Ordering::Relaxed);

        // Cold start: no data yet → use priority only
        if total == 0 {
            return self.config.weight_priority * (slot.priority as f64 / self.slots.len() as f64);
        }

        // Normalized latency: ema / threshold (capped at 2.0)
        let ema_ms = slot.metrics.latency_ema_us.load(Ordering::Relaxed) as f64 / 1000.0;
        let norm_latency = (ema_ms / self.config.latency_threshold_ms as f64).min(2.0);

        // Error rate (0..1)
        let err_rate = slot.metrics.error_rate();

        // Priority score (0..1 based on config order)
        let norm_priority = slot.priority as f64 / self.slots.len().max(1) as f64;

        self.config.weight_latency * norm_latency
            + self.config.weight_error_rate * err_rate
            + self.config.weight_priority * norm_priority
    }

    /// Select provider index and whether this is a probe request.
    fn select_provider(&self) -> (usize, bool) {
        // Score all non-circuit-broken providers
        let mut scored: Vec<(usize, f64)> = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.metrics.is_circuit_open(self.config.failure_threshold))
            .map(|(i, s)| (i, self.score(s)))
            .collect();

        // If all circuit-broken, pick least-failed
        if scored.is_empty() {
            let best = self
                .slots
                .iter()
                .enumerate()
                .min_by_key(|(_, s)| s.metrics.consecutive_failures.load(Ordering::Relaxed))
                .map(|(i, _)| i)
                .unwrap_or(0);
            warn!(
                provider = self.slots[best].provider.provider_name(),
                "all providers circuit-broken, using least-failed"
            );
            return (best, false);
        }

        scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        let best_idx = scored[0].0;

        // Probe: with some probability, redirect to a stale non-primary provider
        if self.slots.len() > 1 && self.should_probe() {
            // Find a stale provider that isn't the best
            for (i, slot) in self.slots.iter().enumerate() {
                if i != best_idx
                    && slot.metrics.is_stale(self.config.probe_interval_secs)
                    && !slot.metrics.is_circuit_open(self.config.failure_threshold)
                {
                    debug!(
                        probe_provider = slot.provider.provider_name(),
                        best_provider = self.slots[best_idx].provider.provider_name(),
                        "probing stale provider"
                    );
                    return (i, true);
                }
            }
        }

        (best_idx, false)
    }

    /// Simple RNG for probe decision.
    fn should_probe(&self) -> bool {
        let state = self.rng_state.load(Ordering::Relaxed);
        // xorshift64
        let mut x = state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng_state.store(x, Ordering::Relaxed);
        let prob = (x % 1000) as f64 / 1000.0;
        prob < self.config.probe_probability
    }

    /// Try a request on a specific provider, returning result and latency.
    async fn try_chat(
        &self,
        idx: usize,
        messages: &[Message],
        tools: &[ToolSpec],
        config: &ChatConfig,
    ) -> Result<ChatResponse> {
        let start = Instant::now();
        let result = self.slots[idx].provider.chat(messages, tools, config).await;
        let elapsed_us = start.elapsed().as_micros() as u64;

        match &result {
            Ok(_) => {
                self.slots[idx]
                    .metrics
                    .record_success_with_alpha(elapsed_us, self.config.ema_alpha);
                let total = self.slots[idx]
                    .metrics
                    .total_requests
                    .load(Ordering::Relaxed);
                if total % 10 == 0 && total > 0 {
                    let snap = self.slots[idx].metrics.snapshot();
                    info!(
                        provider = self.slots[idx].provider.provider_name(),
                        model = self.slots[idx].provider.model_id(),
                        latency_ema_ms = format!("{:.0}", snap.latency_ema_ms),
                        p95_ms = format!("{:.0}", snap.p95_latency_ms),
                        error_rate = format!("{:.1}%", snap.error_rate * 100.0),
                        total_requests = total,
                        "adaptive router metrics"
                    );
                }
            }
            Err(_) => {
                self.slots[idx].metrics.record_failure();
                let consec = self.slots[idx]
                    .metrics
                    .consecutive_failures
                    .load(Ordering::Relaxed);
                if consec == self.config.failure_threshold {
                    warn!(
                        provider = self.slots[idx].provider.provider_name(),
                        consecutive_failures = consec,
                        "provider circuit breaker opened"
                    );
                }
            }
        }

        result
    }

    /// Try a stream request on a specific provider.
    async fn try_chat_stream(
        &self,
        idx: usize,
        messages: &[Message],
        tools: &[ToolSpec],
        config: &ChatConfig,
    ) -> Result<ChatStream> {
        let start = Instant::now();
        let result = self.slots[idx]
            .provider
            .chat_stream(messages, tools, config)
            .await;
        let elapsed_us = start.elapsed().as_micros() as u64;

        match &result {
            Ok(_) => {
                // For streaming, we only measure time-to-first-byte (stream init)
                self.slots[idx]
                    .metrics
                    .record_success_with_alpha(elapsed_us, self.config.ema_alpha);
            }
            Err(_) => {
                self.slots[idx].metrics.record_failure();
            }
        }

        result
    }
}

#[async_trait]
impl LlmProvider for AdaptiveRouter {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        config: &ChatConfig,
    ) -> Result<ChatResponse> {
        let (start_idx, is_probe) = self.select_provider();

        debug!(
            selected = self.slots[start_idx].provider.provider_name(),
            model = self.slots[start_idx].provider.model_id(),
            is_probe = is_probe,
            score = format!("{:.3}", self.score(&self.slots[start_idx])),
            "adaptive router selected provider"
        );

        // Try the selected provider
        match self.try_chat(start_idx, messages, tools, config).await {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                if self.slots.len() == 1 {
                    return Err(e);
                }

                warn!(
                    provider = self.slots[start_idx].provider.provider_name(),
                    error = %e,
                    "adaptive router failing over"
                );

                // Failover: try remaining providers in score order.
                // Any error → try next provider. Don't stop on specific error types.
                let mut scored: Vec<(usize, f64)> = self
                    .slots
                    .iter()
                    .enumerate()
                    .filter(|(i, s)| {
                        *i != start_idx && !s.metrics.is_circuit_open(self.config.failure_threshold)
                    })
                    .map(|(i, s)| (i, self.score(s)))
                    .collect();
                scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

                let mut last_error = e;
                for (idx, _) in scored {
                    match self.try_chat(idx, messages, tools, config).await {
                        Ok(resp) => return Ok(resp),
                        Err(e) => {
                            warn!(
                                provider = self.slots[idx].provider.provider_name(),
                                error = %e,
                                "adaptive router failover also failed"
                            );
                            last_error = e;
                        }
                    }
                }
                Err(last_error)
            }
        }
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        config: &ChatConfig,
    ) -> Result<ChatStream> {
        let (start_idx, _is_probe) = self.select_provider();

        match self
            .try_chat_stream(start_idx, messages, tools, config)
            .await
        {
            Ok(stream) => Ok(stream),
            Err(e) => {
                if self.slots.len() == 1 {
                    return Err(e);
                }

                warn!(
                    provider = self.slots[start_idx].provider.provider_name(),
                    error = %e,
                    "adaptive router failing over stream"
                );

                let mut scored: Vec<(usize, f64)> = self
                    .slots
                    .iter()
                    .enumerate()
                    .filter(|(i, s)| {
                        *i != start_idx && !s.metrics.is_circuit_open(self.config.failure_threshold)
                    })
                    .map(|(i, s)| (i, self.score(s)))
                    .collect();
                scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

                let mut last_error = e;
                for (idx, _) in scored {
                    match self.try_chat_stream(idx, messages, tools, config).await {
                        Ok(stream) => return Ok(stream),
                        Err(e) => {
                            warn!(
                                provider = self.slots[idx].provider.provider_name(),
                                error = %e,
                                "adaptive router failover also failed"
                            );
                            last_error = e;
                        }
                    }
                }
                Err(last_error)
            }
        }
    }

    fn model_id(&self) -> &str {
        let (idx, _) = self.select_provider();
        self.slots[idx].provider.model_id()
    }

    fn provider_name(&self) -> &str {
        let (idx, _) = self.select_provider();
        self.slots[idx].provider.provider_name()
    }

    fn export_metrics(&self) -> Option<serde_json::Value> {
        serde_json::to_value(self.export_shared_metrics()).ok()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_epoch_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{StopReason, TokenUsage};
    use std::sync::Arc;

    struct MockProvider {
        name: &'static str,
        model: &'static str,
        latency_ms: u64,
        fail: bool,
        error_msg: &'static str,
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: &[ToolSpec],
            _config: &ChatConfig,
        ) -> Result<ChatResponse> {
            tokio::time::sleep(std::time::Duration::from_millis(self.latency_ms)).await;
            if self.fail {
                eyre::bail!("{} API error: 429 - rate limited", self.error_msg);
            }
            Ok(ChatResponse {
                content: Some(format!("from-{}", self.name)),
                reasoning_content: None,
                tool_calls: vec![],
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage::default(),
            })
        }

        fn model_id(&self) -> &str {
            self.model
        }

        fn provider_name(&self) -> &str {
            self.name
        }
    }

    #[tokio::test]
    async fn test_selects_primary_on_cold_start() {
        let router = AdaptiveRouter::new(
            vec![
                Arc::new(MockProvider {
                    name: "primary",
                    model: "m1",
                    latency_ms: 0,
                    fail: false,
                    error_msg: "",
                }),
                Arc::new(MockProvider {
                    name: "fallback",
                    model: "m2",
                    latency_ms: 0,
                    fail: false,
                    error_msg: "",
                }),
            ],
            AdaptiveConfig::default(),
        );

        let resp = router.chat(&[], &[], &ChatConfig::default()).await.unwrap();
        assert_eq!(resp.content.unwrap(), "from-primary");
    }

    #[tokio::test]
    async fn test_failover_on_error() {
        let router = AdaptiveRouter::new(
            vec![
                Arc::new(MockProvider {
                    name: "primary",
                    model: "m1",
                    latency_ms: 0,
                    fail: true,
                    error_msg: "Primary",
                }),
                Arc::new(MockProvider {
                    name: "fallback",
                    model: "m2",
                    latency_ms: 0,
                    fail: false,
                    error_msg: "",
                }),
            ],
            AdaptiveConfig::default(),
        );

        let resp = router.chat(&[], &[], &ChatConfig::default()).await.unwrap();
        assert_eq!(resp.content.unwrap(), "from-fallback");
    }

    #[tokio::test]
    async fn test_circuit_breaker_skips_degraded() {
        let config = AdaptiveConfig {
            failure_threshold: 1,
            probe_probability: 0.0, // Disable probes for determinism
            ..Default::default()
        };
        let router = AdaptiveRouter::new(
            vec![
                Arc::new(MockProvider {
                    name: "primary",
                    model: "m1",
                    latency_ms: 0,
                    fail: true,
                    error_msg: "Primary",
                }),
                Arc::new(MockProvider {
                    name: "fallback",
                    model: "m2",
                    latency_ms: 0,
                    fail: false,
                    error_msg: "",
                }),
            ],
            config,
        );

        // First call: primary fails (consecutive_failures=1, trips circuit breaker),
        // failover to fallback succeeds
        let resp = router.chat(&[], &[], &ChatConfig::default()).await.unwrap();
        assert_eq!(resp.content.unwrap(), "from-fallback");

        // Primary is now circuit-broken
        assert!(router.slots[0].metrics.is_circuit_open(1));

        // Second call: should skip primary entirely, go straight to fallback
        let resp = router.chat(&[], &[], &ChatConfig::default()).await.unwrap();
        assert_eq!(resp.content.unwrap(), "from-fallback");
    }

    #[tokio::test]
    async fn test_all_providers_fail() {
        let router = AdaptiveRouter::new(
            vec![
                Arc::new(MockProvider {
                    name: "p1",
                    model: "m1",
                    latency_ms: 0,
                    fail: true,
                    error_msg: "P1",
                }),
                Arc::new(MockProvider {
                    name: "p2",
                    model: "m2",
                    latency_ms: 0,
                    fail: true,
                    error_msg: "P2",
                }),
            ],
            AdaptiveConfig::default(),
        );

        let result = router.chat(&[], &[], &ChatConfig::default()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_metrics_snapshot() {
        let router = AdaptiveRouter::new(
            vec![Arc::new(MockProvider {
                name: "test",
                model: "m1",
                latency_ms: 10,
                fail: false,
                error_msg: "",
            })],
            AdaptiveConfig::default(),
        );

        let _ = router.chat(&[], &[], &ChatConfig::default()).await;

        let snaps = router.metrics_snapshots();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].0, "test");
        assert_eq!(snaps[0].2.success_count, 1);
        assert_eq!(snaps[0].2.failure_count, 0);
        assert!(snaps[0].2.latency_ema_ms > 0.0);
    }

    #[test]
    fn test_scoring_cold_start_respects_priority() {
        let router = AdaptiveRouter::new(
            vec![
                Arc::new(MockProvider {
                    name: "p1",
                    model: "m1",
                    latency_ms: 0,
                    fail: false,
                    error_msg: "",
                }),
                Arc::new(MockProvider {
                    name: "p2",
                    model: "m2",
                    latency_ms: 0,
                    fail: false,
                    error_msg: "",
                }),
            ],
            AdaptiveConfig::default(),
        );

        // On cold start, primary (priority=0) should score lower than fallback (priority=1)
        let score_primary = router.score(&router.slots[0]);
        let score_fallback = router.score(&router.slots[1]);
        assert!(score_primary < score_fallback);
    }

    #[test]
    fn test_latency_samples_p95() {
        let mut samples = LatencySamples::new();
        // Push 100 values: 1..=100
        for i in 1..=100u64 {
            samples.push(i * 1000);
        }
        // p95 of 1..100 should be around 95-96
        let p95 = samples.p95();
        // Buffer is 64 slots, so we have values 37..100
        // p95 of 37..100 = ceil(64*0.95) = 61st value = 97
        assert!(p95 >= 90_000 && p95 <= 100_000, "p95 was {}", p95 / 1000);
    }

    #[test]
    #[should_panic(expected = "at least one provider")]
    fn test_empty_router_panics() {
        let _ = AdaptiveRouter::new(vec![], AdaptiveConfig::default());
    }
}
