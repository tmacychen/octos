//! Standalone deep search tool: web search + parallel page crawling.
//!
//! Reads JSON input from stdin, performs web search, fetches page content,
//! saves results to disk, and outputs JSON to stdout.
//!
//! No crew-agent dependencies. Communicates via stdin/stdout JSON protocol.

use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::time::Duration;

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Input / Output types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Input {
    query: String,
    #[serde(default = "default_max_results")]
    max_results: u8,
    #[serde(default)]
    search_engine: Option<String>,
}

fn default_max_results() -> u8 {
    8
}

#[derive(Serialize)]
struct Output {
    output: String,
    success: bool,
}

// ---------------------------------------------------------------------------
// Search result types (provider-specific)
// ---------------------------------------------------------------------------

// Brave
#[derive(Deserialize)]
struct BraveResponse {
    web: Option<BraveWebResults>,
}

#[derive(Deserialize)]
struct BraveWebResults {
    results: Vec<BraveWebResult>,
}

#[derive(Deserialize)]
struct BraveWebResult {
    title: String,
    url: String,
    description: String,
}

// You.com
#[derive(Deserialize)]
struct YouResponse {
    results: Option<YouResults>,
}

#[derive(Deserialize)]
struct YouResults {
    web: Option<Vec<YouWebResult>>,
}

#[derive(Deserialize)]
struct YouWebResult {
    title: String,
    url: String,
    description: String,
    #[serde(default)]
    snippets: Vec<String>,
}

// Perplexity
#[derive(Deserialize)]
struct PerplexityResponse {
    choices: Option<Vec<PerplexityChoice>>,
    #[serde(default)]
    citations: Vec<String>,
}

#[derive(Deserialize)]
struct PerplexityChoice {
    message: Option<PerplexityMessage>,
}

#[derive(Deserialize)]
struct PerplexityMessage {
    content: Option<String>,
}

// ---------------------------------------------------------------------------
// A single search result (unified)
// ---------------------------------------------------------------------------

struct SearchResult {
    output: String,
    success: bool,
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let mut stdin_buf = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut stdin_buf) {
        print_output(&Output {
            output: format!("Failed to read stdin: {e}"),
            success: false,
        });
        return;
    }

    let input: Input = match serde_json::from_str(&stdin_buf) {
        Ok(v) => v,
        Err(e) => {
            print_output(&Output {
                output: format!("Invalid input JSON: {e}"),
                success: false,
            });
            return;
        }
    };

    let max_results = input.max_results.clamp(1, 10);
    let client = build_client();

    // Step 1: Web search
    let search_result = web_search(
        &client,
        &input.query,
        max_results,
        input.search_engine.as_deref(),
    );

    if !search_result.success {
        print_output(&Output {
            output: search_result.output,
            success: false,
        });
        return;
    }

    // Step 2: Extract URLs from search results
    let urls = extract_urls(&search_result.output);

    if urls.is_empty() {
        print_output(&Output {
            output: search_result.output,
            success: true,
        });
        return;
    }

    // Step 3: Create output directory
    let slug = slugify(&input.query);
    let dir = research_dir(&slug);
    if let Err(e) = fs::create_dir_all(&dir) {
        print_output(&Output {
            output: format!("Failed to create research directory: {e}"),
            success: false,
        });
        return;
    }

    // Step 4: Fetch all pages (sequential in blocking mode)
    let max_chars_per_page: usize = 20_000;
    let pages: Vec<Result<String, String>> = urls
        .iter()
        .map(|url| fetch_page(&client, url, max_chars_per_page))
        .collect();

    // Step 5: Save search results summary
    let search_file = dir.join("_search_results.md");
    let _ = fs::write(&search_file, &search_result.output);

    // Step 6: Save full content to disk, return truncated preview inline
    const INLINE_CHARS_PER_PAGE: usize = 3000;

    let mut output = search_result.output;
    output.push_str("\n---\n\n");

    let mut saved_count = 0u32;
    let mut saved_files = Vec::new();
    for (i, (url, page)) in urls.iter().zip(pages.iter()).enumerate() {
        let filename = format!("{:02}_{}.md", i + 1, host_slug(url));
        let filepath = dir.join(&filename);

        match page {
            Ok(content) if !content.is_empty() => {
                // Save full content to disk
                let page_content = format!("---\nurl: {url}\n---\n\n{content}");
                let _ = fs::write(&filepath, &page_content);
                saved_count += 1;
                saved_files.push(format!("  - {} ({})", filepath.display(), url));

                // Return truncated preview inline to keep context small
                output.push_str(&format!("## Source [{}]: {}\n", i + 1, url));
                output.push_str(&format!("_Full content: {}_\n\n", filepath.display()));
                let preview = truncate_utf8(
                    content,
                    INLINE_CHARS_PER_PAGE,
                    "\n... (truncated, use read_file for full content)",
                );
                output.push_str(&preview);
                output.push_str("\n\n---\n\n");
            }
            Ok(_) => {}
            Err(e) => {
                let err_content = format!("---\nurl: {url}\nerror: {e}\n---\n");
                let _ = fs::write(&filepath, &err_content);
            }
        }
    }

    output.push_str(&format!(
        "{saved_count} pages crawled and saved to: {}\n\nSaved files:\n{}\n\n\
         Use read_file to get full content from specific sources for detailed synthesis.\n",
        dir.display(),
        saved_files.join("\n")
    ));

    print_output(&Output {
        output,
        success: true,
    });
}

// ---------------------------------------------------------------------------
// HTTP client
// ---------------------------------------------------------------------------

fn build_client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
        .build()
        .unwrap_or_else(|_| Client::new())
}

// ---------------------------------------------------------------------------
// Web search (multi-provider)
// ---------------------------------------------------------------------------

fn web_search(client: &Client, query: &str, count: u8, engine: Option<&str>) -> SearchResult {
    // If a specific engine is requested, try it first
    if let Some(eng) = engine {
        match eng {
            "brave" => {
                if let Ok(api_key) = std::env::var("BRAVE_API_KEY") {
                    let r = brave_search(client, query, count, &api_key);
                    if r.success && !r.output.contains("No results found") {
                        return r;
                    }
                }
            }
            "you" => {
                if let Ok(api_key) = std::env::var("YDC_API_KEY") {
                    let r = you_search(client, query, count, &api_key);
                    if r.success && !r.output.contains("No results found") {
                        return r;
                    }
                }
            }
            "perplexity" => {
                if let Ok(api_key) = std::env::var("PERPLEXITY_API_KEY") {
                    let r = perplexity_search(client, query, &api_key);
                    if r.success && !r.output.contains("No results found") {
                        return r;
                    }
                }
            }
            // "duckduckgo" or anything else falls through to default order
            _ => {}
        }
    }

    // Default provider priority: free/cheap first, Perplexity as AI-powered fallback.
    // 1. DuckDuckGo (free, always available)
    // 2. Brave Search (free tier: 2k queries/month)
    // 3. You.com (API key required)
    // 4. Perplexity Sonar (AI-synthesized, most expensive -- fallback only)

    let ddg = ddg_search(client, query, count);
    if ddg.success && !ddg.output.contains("No results found") {
        return ddg;
    }

    if let Ok(api_key) = std::env::var("BRAVE_API_KEY") {
        let r = brave_search(client, query, count, &api_key);
        if r.success && !r.output.contains("No results found") {
            return r;
        }
    }

    if let Ok(api_key) = std::env::var("YDC_API_KEY") {
        let r = you_search(client, query, count, &api_key);
        if r.success && !r.output.contains("No results found") {
            return r;
        }
    }

    if let Ok(api_key) = std::env::var("PERPLEXITY_API_KEY") {
        return perplexity_search(client, query, &api_key);
    }

    // Return whatever DDG gave us (even if empty)
    ddg
}

// ---------------------------------------------------------------------------
// DuckDuckGo HTML search
// ---------------------------------------------------------------------------

fn ddg_search(client: &Client, query: &str, count: u8) -> SearchResult {
    let url = format!("https://html.duckduckgo.com/html/?q={}", urlencoded(query));

    let response = match client.get(&url).send() {
        Ok(r) => r,
        Err(e) => {
            return SearchResult {
                output: format!("DuckDuckGo search error: {e}"),
                success: false,
            };
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        return SearchResult {
            output: format!("DuckDuckGo search error: HTTP {status}"),
            success: false,
        };
    }

    let html = response.text().unwrap_or_default();
    let results = parse_ddg_results(&html, count as usize);

    if results.is_empty() {
        return SearchResult {
            output: format!("No results found for: {query}"),
            success: true,
        };
    }

    let mut output = format!("Results for: {query}\n\n");
    for (i, (title, url, snippet)) in results.iter().enumerate() {
        output.push_str(&format!("{}. {title}\n   {url}\n   {snippet}\n\n", i + 1));
    }

    SearchResult {
        output,
        success: true,
    }
}

/// Parse DuckDuckGo HTML search results.
fn parse_ddg_results(html: &str, max: usize) -> Vec<(String, String, String)> {
    let mut results = Vec::new();
    let marker = "class=\"result__a\"";

    let mut search_from = 0;
    while results.len() < max {
        let pos = match html[search_from..].find(marker) {
            Some(p) => search_from + p + marker.len(),
            None => break,
        };
        search_from = pos;

        let chunk = &html[pos..];

        // Extract href
        let raw_href = match extract_attr(chunk, "href=\"") {
            Some(h) => h,
            None => continue,
        };

        // Decode the real URL from DDG redirect
        let url = decode_ddg_url(&raw_href);
        if !url.starts_with("http") {
            continue;
        }
        if url.contains("duckduckgo.com/y.js") {
            continue;
        }

        // Title is between > and </a>
        let title = match chunk.find('>') {
            Some(gt) => {
                let after = &chunk[gt + 1..];
                match after.find("</a>") {
                    Some(end) => strip_tags(&after[..end]),
                    None => continue,
                }
            }
            None => continue,
        };

        if title.is_empty() {
            continue;
        }

        // Snippet
        let snippet_marker = "class=\"result__snippet\"";
        let snippet = if let Some(sp) = chunk.find(snippet_marker) {
            let after_marker = &chunk[sp + snippet_marker.len()..];
            match after_marker.find('>') {
                Some(gt) => {
                    let content = &after_marker[gt + 1..];
                    match content.find("</a>") {
                        Some(end) => strip_tags(&content[..end]),
                        None => String::new(),
                    }
                }
                None => String::new(),
            }
        } else {
            String::new()
        };

        results.push((title, url, snippet));
    }

    results
}

/// Decode a DuckDuckGo redirect URL to extract the real destination.
fn decode_ddg_url(raw: &str) -> String {
    if let Some(start) = raw.find("uddg=") {
        let encoded = &raw[start + 5..];
        let end = encoded.find('&').unwrap_or(encoded.len());
        urldecoded(&encoded[..end])
    } else if raw.starts_with("http") {
        raw.to_string()
    } else {
        raw.to_string()
    }
}

// ---------------------------------------------------------------------------
// Brave Search
// ---------------------------------------------------------------------------

fn brave_search(client: &Client, query: &str, count: u8, api_key: &str) -> SearchResult {
    let response = match client
        .get("https://api.search.brave.com/res/v1/web/search")
        .header("X-Subscription-Token", api_key)
        .header("Accept", "application/json")
        .query(&[("q", query), ("count", &count.to_string())])
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            return SearchResult {
                output: format!("Brave Search API error: {e}"),
                success: false,
            };
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return SearchResult {
            output: format!("Brave Search API error ({status}): {body}"),
            success: false,
        };
    }

    let brave: BraveResponse = match response.json() {
        Ok(v) => v,
        Err(e) => {
            return SearchResult {
                output: format!("Failed to parse Brave response: {e}"),
                success: false,
            };
        }
    };

    let results = brave.web.map(|w| w.results).unwrap_or_default();

    if results.is_empty() {
        return SearchResult {
            output: format!("No results found for: {query}"),
            success: true,
        };
    }

    let mut output = format!("Results for: {query}\n\n");
    for (i, r) in results.iter().enumerate() {
        output.push_str(&format!(
            "{}. {}\n   {}\n   {}\n\n",
            i + 1,
            r.title,
            r.url,
            r.description
        ));
    }

    SearchResult {
        output,
        success: true,
    }
}

// ---------------------------------------------------------------------------
// You.com Search
// ---------------------------------------------------------------------------

fn you_search(client: &Client, query: &str, count: u8, api_key: &str) -> SearchResult {
    let response = match client
        .get("https://ydc-index.io/v1/search")
        .header("X-API-Key", api_key)
        .query(&[("query", query), ("count", &count.to_string())])
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            return SearchResult {
                output: format!("You.com API error: {e}"),
                success: false,
            };
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return SearchResult {
            output: format!("You.com API error ({status}): {body}"),
            success: false,
        };
    }

    let you: YouResponse = match response.json() {
        Ok(v) => v,
        Err(e) => {
            return SearchResult {
                output: format!("Failed to parse You.com response: {e}"),
                success: false,
            };
        }
    };

    let results = you.results.and_then(|r| r.web).unwrap_or_default();

    if results.is_empty() {
        return SearchResult {
            output: format!("No results found for: {query}"),
            success: true,
        };
    }

    let mut output = format!("Results for: {query}\n\n");
    for (i, r) in results.iter().enumerate() {
        output.push_str(&format!("{}. {}\n   {}\n", i + 1, r.title, r.url));
        if !r.description.is_empty() {
            output.push_str(&format!("   {}\n", r.description));
        }
        if let Some(snippet) = r.snippets.first() {
            let truncated = if snippet.len() > 300 {
                format!("{}...", &snippet[..300])
            } else {
                snippet.clone()
            };
            output.push_str(&format!("   {}\n", truncated));
        }
        output.push('\n');
    }

    SearchResult {
        output,
        success: true,
    }
}

// ---------------------------------------------------------------------------
// Perplexity Sonar Search
// ---------------------------------------------------------------------------

fn perplexity_search(client: &Client, query: &str, api_key: &str) -> SearchResult {
    let body = serde_json::json!({
        "model": "sonar",
        "messages": [{"role": "user", "content": query}],
        "max_tokens": 1024
    });

    let response = match client
        .post("https://api.perplexity.ai/chat/completions")
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            return SearchResult {
                output: format!("Perplexity API error: {e}"),
                success: false,
            };
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return SearchResult {
            output: format!("Perplexity API error ({status}): {body}"),
            success: false,
        };
    }

    let pplx: PerplexityResponse = match response.json() {
        Ok(v) => v,
        Err(e) => {
            return SearchResult {
                output: format!("Failed to parse Perplexity response: {e}"),
                success: false,
            };
        }
    };

    let answer = pplx
        .choices
        .and_then(|c| c.into_iter().next())
        .and_then(|c| c.message)
        .and_then(|m| m.content)
        .unwrap_or_default();

    if answer.is_empty() {
        return SearchResult {
            output: format!("No results found for: {query}"),
            success: true,
        };
    }

    let mut output = format!("Search: {query}\n\n{answer}");

    if !pplx.citations.is_empty() {
        output.push_str("\n\nSources:\n");
        for (i, url) in pplx.citations.iter().enumerate() {
            output.push_str(&format!("  [{}] {}\n", i + 1, url));
        }
    }

    SearchResult {
        output,
        success: true,
    }
}

// ---------------------------------------------------------------------------
// Page fetching
// ---------------------------------------------------------------------------

fn fetch_page(client: &Client, url: &str, max_chars: usize) -> Result<String, String> {
    // Basic SSRF protection: skip private/local addresses
    if let Ok(parsed) = url::Url::parse(url) {
        if let Some(host) = parsed.host_str() {
            if is_private_host(host) {
                return Ok(String::new());
            }
        }
    }

    let response = client
        .get(url)
        .send()
        .map_err(|e| format!("fetch failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }

    let body = response
        .text()
        .map_err(|e| format!("read body failed: {e}"))?;

    // Convert HTML to text using scraper
    let content = html_to_text(&body);
    let truncated = truncate_utf8(&content, max_chars, "\n... (truncated)");

    Ok(truncated)
}

/// Convert HTML to readable text using the scraper crate.
fn html_to_text(html: &str) -> String {
    let document = scraper::Html::parse_document(html);

    // Remove script and style elements
    let mut text_parts: Vec<String> = Vec::new();

    // Use a simple recursive text extraction via ego_tree's NodeRef
    fn extract_text(node: ego_tree::NodeRef<'_, scraper::Node>, parts: &mut Vec<String>) {
        for child in node.children() {
            match child.value() {
                scraper::Node::Text(text) => {
                    let t = text.trim();
                    if !t.is_empty() {
                        parts.push(t.to_string());
                    }
                }
                scraper::Node::Element(el) => {
                    let tag = el.name();
                    // Skip script and style content
                    if tag == "script" || tag == "style" || tag == "noscript" {
                        continue;
                    }
                    // Add newlines around block elements
                    let is_block = matches!(
                        tag,
                        "p" | "div"
                            | "h1"
                            | "h2"
                            | "h3"
                            | "h4"
                            | "h5"
                            | "h6"
                            | "li"
                            | "tr"
                            | "br"
                            | "hr"
                            | "blockquote"
                            | "pre"
                            | "section"
                            | "article"
                            | "header"
                            | "footer"
                            | "nav"
                            | "main"
                            | "aside"
                    );
                    if is_block {
                        parts.push("\n".to_string());
                    }
                    extract_text(child, parts);
                    if is_block {
                        parts.push("\n".to_string());
                    }
                }
                _ => {}
            }
        }
    }

    extract_text(document.tree.root(), &mut text_parts);

    let raw = text_parts.join(" ");

    // Clean up excessive whitespace
    let mut result = String::with_capacity(raw.len());
    let mut prev_newline = false;
    let mut prev_space = false;
    for ch in raw.chars() {
        if ch == '\n' {
            if !prev_newline {
                result.push('\n');
            }
            prev_newline = true;
            prev_space = false;
        } else if ch.is_whitespace() {
            if !prev_space && !prev_newline {
                result.push(' ');
            }
            prev_space = true;
        } else {
            prev_newline = false;
            prev_space = false;
            result.push(ch);
        }
    }

    result.trim().to_string()
}

/// Simple text extraction fallback (strip HTML tags).
#[allow(dead_code)]
fn extract_text_simple(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        if c == '<' {
            in_tag = true;
            continue;
        }
        if c == '>' {
            in_tag = false;
            result.push(' ');
            continue;
        }
        if !in_tag {
            result.push(c);
        }
    }
    result.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------
// SSRF protection
// ---------------------------------------------------------------------------

fn is_private_host(host: &str) -> bool {
    if host == "localhost"
        || host == "127.0.0.1"
        || host == "::1"
        || host == "0.0.0.0"
        || host.ends_with(".local")
        || host.ends_with(".internal")
    {
        return true;
    }

    // Check for private IP ranges
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return is_private_ip(&ip);
    }

    false
}

fn is_private_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                // 100.64.0.0/10 (CGNAT)
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64)
        }
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                // ULA: fc00::/7
                || (v6.octets()[0] & 0xfe) == 0xfc
                // Link-local: fe80::/10
                || (v6.octets()[0] == 0xfe && (v6.octets()[1] & 0xc0) == 0x80)
        }
    }
}

// ---------------------------------------------------------------------------
// URL helpers
// ---------------------------------------------------------------------------

/// Simple URL encoding for query parameters.
fn urlencoded(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(char::from(HEX[(b >> 4) as usize]));
                out.push(char::from(HEX[(b & 0xf) as usize]));
            }
        }
    }
    out
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

fn urldecoded(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hi = bytes.next().and_then(hex_val);
            let lo = bytes.next().and_then(hex_val);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h << 4 | l) as char);
            }
        } else {
            out.push(b as char);
        }
    }
    out
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'A'..=b'F' => Some(b - b'A' + 10),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

/// Extract an attribute value after the given prefix (up to the next `"`).
fn extract_attr(html: &str, prefix: &str) -> Option<String> {
    let start = html.find(prefix)? + prefix.len();
    let end = html[start..].find('"')? + start;
    Some(decode_html_entities(&html[start..end]))
}

/// Strip HTML tags from a string.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    decode_html_entities(out.trim())
}

/// Decode common HTML entities.
fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

// ---------------------------------------------------------------------------
// URL extraction from search output
// ---------------------------------------------------------------------------

fn extract_urls(output: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            urls.push(trimmed.to_string());
        }
        // "[N] url" format from Perplexity citations
        if let Some(rest) = trimmed.strip_prefix('[') {
            if let Some(after_bracket) = rest.find("] ") {
                let url = &rest[after_bracket + 2..];
                if url.starts_with("http") {
                    urls.push(url.to_string());
                }
            }
        }
    }
    urls
}

// ---------------------------------------------------------------------------
// String helpers
// ---------------------------------------------------------------------------

/// Convert a query string to a filesystem-safe slug.
fn slugify(s: &str) -> String {
    let mut slug = String::with_capacity(s.len());
    for ch in s.chars().take(60) {
        if ch.is_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if ch == ' ' || ch == '-' || ch == '_' {
            if !slug.ends_with('-') {
                slug.push('-');
            }
        }
    }
    slug.trim_matches('-').to_string()
}

/// Extract a short slug from a URL's hostname.
fn host_slug(raw_url: &str) -> String {
    url::Url::parse(raw_url)
        .ok()
        .and_then(|u| {
            u.host_str()
                .map(|h| h.strip_prefix("www.").unwrap_or(h).replace('.', "-"))
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// Truncate a string to max_chars at a UTF-8 safe boundary, appending suffix if truncated.
fn truncate_utf8(s: &str, max_chars: usize, suffix: &str) -> String {
    if s.len() <= max_chars {
        return s.to_string();
    }

    // Find a valid UTF-8 boundary at or before max_chars
    let mut end = max_chars;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }

    let mut result = s[..end].to_string();
    result.push_str(suffix);
    result
}

/// Get the research directory path.
fn research_dir(slug: &str) -> PathBuf {
    PathBuf::from("./research").join(slug)
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

fn print_output(output: &Output) {
    let json = serde_json::to_string(output).unwrap_or_else(|_| {
        r#"{"output":"Failed to serialize output","success":false}"#.to_string()
    });
    println!("{json}");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("top AI startups 2025"), "top-ai-startups-2025");
        assert_eq!(slugify("NVIDIA stock price!"), "nvidia-stock-price");
        assert_eq!(slugify("  spaces  "), "spaces");
    }

    #[test]
    fn test_host_slug() {
        assert_eq!(host_slug("https://www.example.com/page"), "example-com");
        assert_eq!(host_slug("https://api.you.com/search"), "api-you-com");
        assert_eq!(host_slug("https://nerdwallet.com"), "nerdwallet-com");
    }

    #[test]
    fn test_extract_urls_from_search_results() {
        let output = "Results for: test\n\n1. Title\n   https://example.com/page\n   Description\n\n2. Title 2\n   https://other.com\n   Desc\n";
        let urls = extract_urls(output);
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0], "https://example.com/page");
        assert_eq!(urls[1], "https://other.com");
    }

    #[test]
    fn test_extract_urls_from_perplexity_citations() {
        let output =
            "Answer text\n\nSources:\n  [1] https://example.com\n  [2] https://other.com\n";
        let urls = extract_urls(output);
        assert_eq!(urls.len(), 2);
    }

    #[test]
    fn test_extract_urls_empty() {
        assert!(extract_urls("no urls here").is_empty());
    }

    #[test]
    fn test_truncate_utf8() {
        let s = "Hello, world!";
        assert_eq!(truncate_utf8(s, 100, "..."), "Hello, world!");
        assert_eq!(truncate_utf8(s, 5, "..."), "Hello...");
    }

    #[test]
    fn test_truncate_utf8_multibyte() {
        let s = "Hello \u{1F600} world";
        let result = truncate_utf8(s, 7, "...");
        assert!(result.is_char_boundary(result.len()));
        // Should truncate before the emoji since the emoji is at byte 6..10
        assert_eq!(result, "Hello \u{1F600}...");
    }

    #[test]
    fn test_is_private_host() {
        assert!(is_private_host("localhost"));
        assert!(is_private_host("127.0.0.1"));
        assert!(is_private_host("::1"));
        assert!(is_private_host("0.0.0.0"));
        assert!(is_private_host("foo.local"));
        assert!(!is_private_host("example.com"));
        assert!(!is_private_host("8.8.8.8"));
    }

    #[test]
    fn test_is_private_ip() {
        assert!(is_private_ip(&"127.0.0.1".parse().unwrap()));
        assert!(is_private_ip(&"10.0.0.1".parse().unwrap()));
        assert!(is_private_ip(&"192.168.1.1".parse().unwrap()));
        assert!(is_private_ip(&"172.16.0.1".parse().unwrap()));
        assert!(!is_private_ip(&"8.8.8.8".parse().unwrap()));
        assert!(!is_private_ip(&"1.1.1.1".parse().unwrap()));
    }

    #[test]
    fn test_urlencoded() {
        assert_eq!(urlencoded("hello world"), "hello+world");
        assert_eq!(urlencoded("a&b=c"), "a%26b%3Dc");
    }

    #[test]
    fn test_strip_tags() {
        assert_eq!(strip_tags("<b>hello</b> world"), "hello world");
        assert_eq!(strip_tags("no tags"), "no tags");
    }

    #[test]
    fn test_decode_html_entities() {
        assert_eq!(decode_html_entities("a &amp; b"), "a & b");
        assert_eq!(decode_html_entities("1 &lt; 2"), "1 < 2");
    }

    #[test]
    fn test_parse_ddg_results_empty() {
        assert!(parse_ddg_results("", 5).is_empty());
        assert!(parse_ddg_results("<html>no results</html>", 5).is_empty());
    }

    #[test]
    fn test_parse_ddg_results_basic() {
        let html = r#"<a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpage&amp;rut=abc123">Example Title</a><a class="result__snippet">This is a snippet.</a>"#;
        let results = parse_ddg_results(html, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "Example Title");
        assert_eq!(results[0].1, "https://example.com/page");
        assert_eq!(results[0].2, "This is a snippet.");
    }

    #[test]
    fn test_html_to_text_basic() {
        let html = "<html><body><h1>Title</h1><p>Hello world</p></body></html>";
        let text = html_to_text(html);
        assert!(text.contains("Title"));
        assert!(text.contains("Hello world"));
    }

    #[test]
    fn test_html_to_text_strips_script() {
        let html = "<html><body><script>var x = 1;</script><p>Content</p></body></html>";
        let text = html_to_text(html);
        assert!(!text.contains("var x"));
        assert!(text.contains("Content"));
    }

    #[test]
    fn test_output_json_format() {
        let output = Output {
            output: "test result".to_string(),
            success: true,
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"output\""));
        assert!(json.contains("\"success\":true"));
    }

    #[test]
    fn test_input_deserialization_defaults() {
        let json = r#"{"query": "test"}"#;
        let input: Input = serde_json::from_str(json).unwrap();
        assert_eq!(input.query, "test");
        assert_eq!(input.max_results, 8);
        assert!(input.search_engine.is_none());
    }

    #[test]
    fn test_input_deserialization_full() {
        let json = r#"{"query": "test", "max_results": 3, "search_engine": "brave"}"#;
        let input: Input = serde_json::from_str(json).unwrap();
        assert_eq!(input.query, "test");
        assert_eq!(input.max_results, 3);
        assert_eq!(input.search_engine.as_deref(), Some("brave"));
    }
}
