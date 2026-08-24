use okf_validator::{Language, ParseSeverityError, Severity};

#[test]
fn test_severity_traits() {
    assert_eq!(Severity::Info.as_str(), "info");
    assert_eq!(Severity::Warning.as_ref(), "warning");
    assert_eq!(Severity::Error.to_string(), "error");

    assert_eq!("info".parse::<Severity>(), Ok(Severity::Info));
    assert_eq!("warning".parse::<Severity>(), Ok(Severity::Warning));
    assert_eq!("warn".parse::<Severity>(), Ok(Severity::Warning));
    assert_eq!("error".parse::<Severity>(), Ok(Severity::Error));
    assert_eq!(
        "invalid".parse::<Severity>(),
        Err(ParseSeverityError("invalid".into()))
    );

    // Ordering: Info < Warning < Error
    assert!(Severity::Info < Severity::Warning);
    assert!(Severity::Warning < Severity::Error);
    assert!(Severity::Error >= Severity::Warning);
}

#[test]
fn test_language_traits() {
    assert_eq!(Language::Python.as_str(), "python");
    assert_eq!(Language::JavaScript.as_ref(), "javascript");
    assert_eq!(Language::TypeScript.to_string(), "typescript");

    let parsed: Language = "python".parse().unwrap();
    assert_eq!(parsed, Language::Python);
    let parsed_rs: Language = "rust".parse().unwrap();
    assert_eq!(parsed_rs, Language::Rust);
    let parsed_unknown: Language = "nonexistent_lang".parse().unwrap();
    assert_eq!(parsed_unknown, Language::Unknown);
}
