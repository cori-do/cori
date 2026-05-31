//! Phase 1 security-critical tests:
//!   * non-loopback bind is refused
//!   * cookie parser handles multi-pair `Cookie` headers
//!   * path-segment guard rejects traversal / separators / control chars

use std::net::SocketAddr;

#[tokio::test]
async fn serve_refuses_non_loopback() {
    let addr: SocketAddr = "0.0.0.0:0".parse().unwrap();
    let res = cori_console::serve_at(addr, "test-token".to_string(), std::env::temp_dir()).await;
    let err = res.unwrap_err().to_string();
    assert!(
        err.contains("non-loopback"),
        "expected loopback rejection, got: {err}"
    );
}

#[test]
fn cookie_parser_picks_named_value() {
    use cori_console::auth::cookie_value;
    let header = "other=foo; cori_session=abc123; trailing=bar";
    assert_eq!(
        cookie_value(header, "cori_session"),
        Some("abc123".to_string())
    );
    assert_eq!(cookie_value(header, "other"), Some("foo".to_string()));
    assert_eq!(cookie_value(header, "missing"), None);
    assert_eq!(cookie_value("", "cori_session"), None);
}

#[test]
fn safe_segment_rejects_traversal_and_separators() {
    use cori_console::api::runs::is_safe_segment;
    assert!(is_safe_segment("translate_fr-abcd1234"));
    assert!(is_safe_segment("2026-05-31T12-00-00Z.json"));
    assert!(!is_safe_segment(""));
    assert!(!is_safe_segment("."));
    assert!(!is_safe_segment(".."));
    assert!(!is_safe_segment("foo/bar"));
    assert!(!is_safe_segment("foo\\bar"));
    assert!(!is_safe_segment("foo\0bar"));
    assert!(!is_safe_segment("foo\nbar"));
}

#[test]
fn tokens_are_distinct_and_long_enough() {
    let a = cori_console::generate_token();
    let b = cori_console::generate_token();
    assert_ne!(a, b);
    // 32 bytes → 43 base64url chars (no padding).
    assert_eq!(a.len(), 43);
    assert_eq!(b.len(), 43);
    let s = cori_console::generate_session_value();
    assert_ne!(s, a);
    assert_eq!(s.len(), 43);
}
