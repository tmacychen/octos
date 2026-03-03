//! Embedded metadata for app-skill binaries that ship alongside the `crew` binary.
//!
//! Each entry contains: (dir_name, binary_name, SKILL.md content, manifest.json content).
//! The actual binaries are sibling executables in the same directory as the `crew` binary;
//! [`super::bootstrap`] copies them into `.crew/skills/` at gateway startup.

/// (dir_name, binary_name, skill_md, manifest_json)
pub const BUNDLED_APP_SKILLS: &[(&str, &str, &str, &str)] = &[
    (
        "news",
        "news_fetch",
        include_str!("../../app-skills/news/SKILL.md"),
        include_str!("../../app-skills/news/manifest.json"),
    ),
    (
        "deep-search",
        "deep-search",
        include_str!("../../app-skills/deep-search/SKILL.md"),
        include_str!("../../app-skills/deep-search/manifest.json"),
    ),
    (
        "deep-crawl",
        "deep_crawl",
        include_str!("../../app-skills/deep-crawl/SKILL.md"),
        include_str!("../../app-skills/deep-crawl/manifest.json"),
    ),
    (
        "send-email",
        "send_email",
        include_str!("../../app-skills/send-email/SKILL.md"),
        include_str!("../../app-skills/send-email/manifest.json"),
    ),
    (
        "account-manager",
        "account_manager",
        include_str!("../../app-skills/account-manager/SKILL.md"),
        include_str!("../../app-skills/account-manager/manifest.json"),
    ),
];
