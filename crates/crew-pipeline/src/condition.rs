//! Condition expression parser and evaluator for edge selection.
//!
//! Grammar:
//! ```text
//! expr     = or_expr
//! or_expr  = and_expr ("||" and_expr)*
//! and_expr = unary ("&&" unary)*
//! unary    = "!" unary | primary
//! primary  = "(" expr ")"
//!          | "outcome.status" ("==" | "!=") STRING
//!          | "outcome.contains(" STRING ")"
//!          | STRING   // bare literal, compared to outcome.content
//! ```

use eyre::Result;

use crate::graph::NodeOutcome;

/// A parsed condition expression.
#[derive(Debug, Clone)]
pub enum CondExpr {
    /// `outcome.status == "pass"`
    StatusEq(String),
    /// `outcome.status != "fail"`
    StatusNe(String),
    /// `outcome.contains("keyword")`
    Contains(String),
    /// `!expr`
    Not(Box<CondExpr>),
    /// `expr && expr`
    And(Box<CondExpr>, Box<CondExpr>),
    /// `expr || expr`
    Or(Box<CondExpr>, Box<CondExpr>),
    /// Always true
    True,
}

/// Parse a condition expression string.
pub fn parse_condition(input: &str) -> Result<CondExpr> {
    let tokens = tokenize(input)?;
    let mut parser = ExprParser::new(&tokens);
    let expr = parser.parse_or()?;
    if parser.pos < parser.tokens.len() {
        eyre::bail!(
            "unexpected token at position {}: {:?}",
            parser.pos,
            parser.tokens[parser.pos]
        );
    }
    Ok(expr)
}

/// Evaluate a condition expression against a node outcome.
pub fn evaluate(expr: &CondExpr, outcome: &NodeOutcome) -> bool {
    match expr {
        CondExpr::StatusEq(s) => status_str(outcome) == s.as_str(),
        CondExpr::StatusNe(s) => status_str(outcome) != s.as_str(),
        CondExpr::Contains(s) => outcome.content.contains(s.as_str()),
        CondExpr::Not(inner) => !evaluate(inner, outcome),
        CondExpr::And(a, b) => evaluate(a, outcome) && evaluate(b, outcome),
        CondExpr::Or(a, b) => evaluate(a, outcome) || evaluate(b, outcome),
        CondExpr::True => true,
    }
}

fn status_str(outcome: &NodeOutcome) -> &str {
    match outcome.status {
        crate::graph::OutcomeStatus::Pass => "pass",
        crate::graph::OutcomeStatus::Fail => "fail",
        crate::graph::OutcomeStatus::Error => "error",
    }
}

// ---- Tokenizer ----

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    StringLit(String),
    Eq,     // ==
    Ne,     // !=
    And,    // &&
    Or,     // ||
    Not,    // !
    LParen, // (
    RParen, // )
    Dot,    // .
}

fn tokenize(input: &str) -> Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' | b'\n' | b'\r' => i += 1,
            b'(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            b')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            b'.' => {
                tokens.push(Token::Dot);
                i += 1;
            }
            b'=' if i + 1 < bytes.len() && bytes[i + 1] == b'=' => {
                tokens.push(Token::Eq);
                i += 2;
            }
            b'!' if i + 1 < bytes.len() && bytes[i + 1] == b'=' => {
                tokens.push(Token::Ne);
                i += 2;
            }
            b'!' => {
                tokens.push(Token::Not);
                i += 1;
            }
            b'&' if i + 1 < bytes.len() && bytes[i + 1] == b'&' => {
                tokens.push(Token::And);
                i += 2;
            }
            b'|' if i + 1 < bytes.len() && bytes[i + 1] == b'|' => {
                tokens.push(Token::Or);
                i += 2;
            }
            b'"' => {
                i += 1;
                let start = i;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1; // skip escaped char
                    }
                    i += 1;
                }
                let s = String::from_utf8_lossy(&bytes[start..i]).to_string();
                tokens.push(Token::StringLit(s));
                if i < bytes.len() {
                    i += 1; // skip closing quote
                }
            }
            c if c.is_ascii_alphanumeric() || c == b'_' => {
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let s = String::from_utf8_lossy(&bytes[start..i]).to_string();
                tokens.push(Token::Ident(s));
            }
            c => eyre::bail!("unexpected character '{}' at position {}", c as char, i),
        }
    }

    Ok(tokens)
}

// ---- Recursive descent parser ----

struct ExprParser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> ExprParser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<&Token> {
        let tok = self.tokens.get(self.pos)?;
        self.pos += 1;
        Some(tok)
    }

    fn expect_token(&mut self, expected: &Token) -> Result<()> {
        match self.advance() {
            Some(tok) if tok == expected => Ok(()),
            Some(tok) => eyre::bail!("expected {:?}, found {:?}", expected, tok),
            None => eyre::bail!("expected {:?}, found EOF", expected),
        }
    }

    fn parse_or(&mut self) -> Result<CondExpr> {
        let mut left = self.parse_and()?;
        while self.peek() == Some(&Token::Or) {
            self.advance();
            let right = self.parse_and()?;
            left = CondExpr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<CondExpr> {
        let mut left = self.parse_unary()?;
        while self.peek() == Some(&Token::And) {
            self.advance();
            let right = self.parse_unary()?;
            left = CondExpr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<CondExpr> {
        if self.peek() == Some(&Token::Not) {
            self.advance();
            let inner = self.parse_unary()?;
            return Ok(CondExpr::Not(Box::new(inner)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<CondExpr> {
        // Parenthesized expression
        if self.peek() == Some(&Token::LParen) {
            self.advance();
            let expr = self.parse_or()?;
            self.expect_token(&Token::RParen)?;
            return Ok(expr);
        }

        // `outcome.status == "..."` or `outcome.contains("...")`
        if self.peek() == Some(&Token::Ident("outcome".into())) {
            self.advance();
            self.expect_token(&Token::Dot)?;

            match self.advance() {
                Some(Token::Ident(field)) if field == "status" => match self.advance() {
                    Some(Token::Eq) => {
                        let val = self.expect_string()?;
                        Ok(CondExpr::StatusEq(val))
                    }
                    Some(Token::Ne) => {
                        let val = self.expect_string()?;
                        Ok(CondExpr::StatusNe(val))
                    }
                    other => eyre::bail!("expected == or != after outcome.status, got {:?}", other),
                },
                Some(Token::Ident(field)) if field == "contains" => {
                    self.expect_token(&Token::LParen)?;
                    let val = self.expect_string()?;
                    self.expect_token(&Token::RParen)?;
                    Ok(CondExpr::Contains(val))
                }
                other => eyre::bail!("unknown outcome field: {:?}", other),
            }
        } else {
            eyre::bail!("expected 'outcome' or '(', got {:?}", self.peek())
        }
    }

    fn expect_string(&mut self) -> Result<String> {
        match self.advance() {
            Some(Token::StringLit(s)) => Ok(s.clone()),
            other => eyre::bail!("expected string literal, got {:?}", other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{NodeOutcome, OutcomeStatus};
    use crew_core::TokenUsage;

    fn outcome(status: OutcomeStatus, content: &str) -> NodeOutcome {
        NodeOutcome {
            node_id: "test".into(),
            status,
            content: content.into(),
            token_usage: TokenUsage::default(),
        }
    }

    #[test]
    fn test_status_eq() {
        let expr = parse_condition(r#"outcome.status == "pass""#).unwrap();
        assert!(evaluate(&expr, &outcome(OutcomeStatus::Pass, "")));
        assert!(!evaluate(&expr, &outcome(OutcomeStatus::Fail, "")));
    }

    #[test]
    fn test_status_ne() {
        let expr = parse_condition(r#"outcome.status != "fail""#).unwrap();
        assert!(evaluate(&expr, &outcome(OutcomeStatus::Pass, "")));
        assert!(!evaluate(&expr, &outcome(OutcomeStatus::Fail, "")));
    }

    #[test]
    fn test_contains() {
        let expr = parse_condition(r#"outcome.contains("error")"#).unwrap();
        assert!(evaluate(
            &expr,
            &outcome(OutcomeStatus::Pass, "found an error")
        ));
        assert!(!evaluate(&expr, &outcome(OutcomeStatus::Pass, "all good")));
    }

    #[test]
    fn test_not() {
        let expr = parse_condition(r#"!outcome.status == "fail""#).unwrap();
        assert!(evaluate(&expr, &outcome(OutcomeStatus::Pass, "")));
        assert!(!evaluate(&expr, &outcome(OutcomeStatus::Fail, "")));
    }

    #[test]
    fn test_and() {
        let expr =
            parse_condition(r#"outcome.status == "pass" && outcome.contains("done")"#).unwrap();
        assert!(evaluate(&expr, &outcome(OutcomeStatus::Pass, "task done")));
        assert!(!evaluate(&expr, &outcome(OutcomeStatus::Pass, "not yet")));
        assert!(!evaluate(&expr, &outcome(OutcomeStatus::Fail, "done")));
    }

    #[test]
    fn test_or() {
        let expr =
            parse_condition(r#"outcome.status == "pass" || outcome.status == "fail""#).unwrap();
        assert!(evaluate(&expr, &outcome(OutcomeStatus::Pass, "")));
        assert!(evaluate(&expr, &outcome(OutcomeStatus::Fail, "")));
        assert!(!evaluate(&expr, &outcome(OutcomeStatus::Error, "")));
    }

    #[test]
    fn test_parentheses() {
        let expr =
            parse_condition(r#"!(outcome.status == "error") && outcome.contains("ok")"#).unwrap();
        assert!(evaluate(&expr, &outcome(OutcomeStatus::Pass, "all ok")));
        assert!(!evaluate(&expr, &outcome(OutcomeStatus::Error, "ok")));
    }
}
