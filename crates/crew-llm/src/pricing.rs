//! Model pricing for cost estimation.

/// Pricing per 1M tokens (input, output) in USD.
#[derive(Debug, Clone, Copy)]
pub struct ModelPricing {
    pub input_per_million: f64,
    pub output_per_million: f64,
}

impl ModelPricing {
    /// Calculate cost for given token counts.
    pub fn cost(&self, input_tokens: u32, output_tokens: u32) -> f64 {
        (input_tokens as f64 / 1_000_000.0) * self.input_per_million
            + (output_tokens as f64 / 1_000_000.0) * self.output_per_million
    }
}

/// Look up pricing for a model. Returns None for unknown/local models.
pub fn model_pricing(model_id: &str) -> Option<ModelPricing> {
    let m = model_id.to_lowercase();

    // Anthropic
    if m.contains("claude-opus-4") || m.contains("claude-4-opus") {
        return Some(ModelPricing {
            input_per_million: 15.0,
            output_per_million: 75.0,
        });
    }
    if m.contains("claude-sonnet-4") || m.contains("claude-4-sonnet") {
        return Some(ModelPricing {
            input_per_million: 3.0,
            output_per_million: 15.0,
        });
    }
    if m.contains("claude-3-5-sonnet") {
        return Some(ModelPricing {
            input_per_million: 3.0,
            output_per_million: 15.0,
        });
    }
    if m.contains("claude-3-5-haiku") || m.contains("claude-haiku") {
        return Some(ModelPricing {
            input_per_million: 0.80,
            output_per_million: 4.0,
        });
    }

    // OpenAI
    if m.contains("gpt-4o-mini") {
        return Some(ModelPricing {
            input_per_million: 0.15,
            output_per_million: 0.60,
        });
    }
    if m.contains("gpt-4o") {
        return Some(ModelPricing {
            input_per_million: 2.50,
            output_per_million: 10.0,
        });
    }
    if m.starts_with("o3") || m.starts_with("o4") {
        return Some(ModelPricing {
            input_per_million: 10.0,
            output_per_million: 40.0,
        });
    }

    // Gemini
    if m.contains("gemini-2") || m.contains("gemini-1.5") {
        return Some(ModelPricing {
            input_per_million: 0.075,
            output_per_million: 0.30,
        });
    }

    // DeepSeek
    if m.contains("deepseek") {
        return Some(ModelPricing {
            input_per_million: 0.27,
            output_per_million: 1.10,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_known_model_pricing() {
        let p = model_pricing("claude-sonnet-4-20250514").unwrap();
        assert!((p.input_per_million - 3.0).abs() < f64::EPSILON);
        assert!((p.output_per_million - 15.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_cost_calculation() {
        let p = ModelPricing {
            input_per_million: 3.0,
            output_per_million: 15.0,
        };
        let cost = p.cost(1_000_000, 100_000);
        // $3.00 input + $1.50 output = $4.50
        assert!((cost - 4.5).abs() < 0.001);
    }

    #[test]
    fn test_gpt4o_mini_before_gpt4o() {
        // gpt-4o-mini must match before gpt-4o
        let mini = model_pricing("gpt-4o-mini").unwrap();
        assert!((mini.input_per_million - 0.15).abs() < f64::EPSILON);
        let full = model_pricing("gpt-4o").unwrap();
        assert!((full.input_per_million - 2.50).abs() < f64::EPSILON);
    }

    #[test]
    fn test_unknown_model_returns_none() {
        assert!(model_pricing("my-local-model").is_none());
        assert!(model_pricing("ollama/llama3").is_none());
    }
}
