use okf_core::{
    Actor, ActorKind, ConceptId, Document, DocumentError, Frontmatter, LinkKind, Mapping,
    ParseActorKindError, ParseLinkKindError, ParseTrustTierError, Status, TrustTier, Value,
};
use std::borrow::Cow;
use std::str::FromStr;

#[test]
fn test_document_from_str_and_display() {
    let src = "---\ntype: Metric\ntitle: Revenue\n---\n\n# Heading\n\nBody text.\n";
    let doc: Document = src.parse().expect("document parse via FromStr");
    assert_eq!(doc.frontmatter.type_().as_deref(), Some("Metric"));
    assert_eq!(doc.frontmatter.title().as_deref(), Some("Revenue"));
    assert_eq!(doc.to_string(), doc.serialize());

    let bad_src = "---\ntype: Metric\n";
    let err = Document::from_str(bad_src).unwrap_err();
    assert_eq!(err, DocumentError::UnterminatedFrontmatter);
}

#[test]
fn test_frontmatter_traits_and_methods() {
    let mut fm = Frontmatter::new();
    assert!(fm.is_empty());
    assert_eq!(fm.len(), 0);

    fm.set("type", "Metric".into());
    fm.set("title", "Revenue".into());
    assert!(!fm.is_empty());
    assert_eq!(fm.len(), 2);
    assert!(fm.contains_key("type"));
    assert!(fm.contains_key("title"));
    assert!(!fm.contains_key("description"));

    let keys: Vec<&str> = fm.keys().collect();
    assert_eq!(keys, vec!["type", "title"]);

    let values: Vec<&Value> = fm.values().collect();
    assert_eq!(values.len(), 2);

    let pairs: Vec<(&Value, &Value)> = fm.iter().collect();
    assert_eq!(pairs.len(), 2);

    // Display
    let fm_str = fm.to_string();
    assert!(fm_str.contains("type: Metric"));

    // FromStr
    let parsed: Frontmatter = "type: Metric\ntitle: Revenue\n".parse().unwrap();
    assert_eq!(parsed.type_().as_deref(), Some("Metric"));
    assert_eq!(parsed.title().as_deref(), Some("Revenue"));

    // From / Into Mapping
    let mapping: Mapping = fm.clone().into();
    assert_eq!(mapping.len(), 2);
    let fm_back = Frontmatter::from(mapping);
    assert_eq!(fm_back, fm);

    // FromIterator
    let iter_fm: Frontmatter = vec![
        ("type".to_string(), Value::from("Table")),
        ("title".to_string(), Value::from("Orders")),
    ]
    .into_iter()
    .collect();
    assert_eq!(iter_fm.len(), 2);

    let str_iter_fm: Frontmatter = vec![("type", Value::from("Table"))].into_iter().collect();
    assert_eq!(str_iter_fm.len(), 1);

    // Extend
    let mut extended = Frontmatter::new();
    extended.extend(vec![("type".to_string(), Value::from("Table"))]);
    extended.extend(vec![("title", Value::from("Orders"))]);
    assert_eq!(extended.len(), 2);
}

#[test]
fn test_mapping_traits() {
    let mut map = Mapping::new();
    map.insert("a", 1.into());
    map.insert("b", 2.into());

    assert_eq!(map.len(), 2);
    assert_eq!(map.entries().len(), 2);

    let values: Vec<&Value> = map.values().collect();
    assert_eq!(values.len(), 2);

    // Display
    assert!(map.to_string().contains("a: 1"));

    // IntoIterator by ref
    let mut count = 0;
    for (k, v) in &map {
        assert!(k.as_str().is_some());
        assert!(v.as_int().is_some());
        count += 1;
    }
    assert_eq!(count, 2);

    // IntoIterator by mut ref
    for (_k, v) in &mut map {
        if let Value::Int(i) = v {
            *i += 10;
        }
    }
    assert_eq!(map.get("a"), Some(&Value::Int(11)));

    // IntoIterator by value
    let entries: Vec<(Value, Value)> = map.into_iter().collect();
    assert_eq!(entries.len(), 2);

    // FromIterator & Extend
    let mut new_map: Mapping = vec![("x", Value::from(42))].into_iter().collect();
    new_map.extend(vec![("y".to_string(), Value::from(99))]);
    assert_eq!(new_map.len(), 2);
}

#[test]
fn test_value_conversions_and_methods() {
    // as_float
    let vf = Value::Float(3.14);
    assert_eq!(vf.as_float(), Some(3.14));
    assert_eq!(Value::Int(42).as_float(), None);

    // FromStr
    let val: Value = "key: [1, 2, 3]".parse().unwrap();
    assert!(matches!(val, Value::Mapping(_)));

    // Numeric primitive conversions
    assert_eq!(Value::from(42_i32), Value::Int(42));
    assert_eq!(Value::from(10_i16), Value::Int(10));
    assert_eq!(Value::from(5_i8), Value::Int(5));
    assert_eq!(Value::from(100_u32), Value::Int(100));
    assert_eq!(Value::from(20_u16), Value::Int(20));
    assert_eq!(Value::from(8_u8), Value::Int(8));
    assert_eq!(Value::from(50_u64), Value::Int(50));
    assert_eq!(Value::from(7_usize), Value::Int(7));

    // Float conversions
    assert_eq!(Value::from(2.718_f64), Value::Float(2.718));
    assert_eq!(Value::from(1.5_f32), Value::Float(1.5));

    // String / Cow conversions
    let owned = "test".to_string();
    assert_eq!(Value::from(&owned), Value::String("test".to_string()));
    assert_eq!(
        Value::from(Cow::Borrowed("borrowed")),
        Value::String("borrowed".to_string())
    );

    // Option and unit conversions
    assert_eq!(Value::from(()), Value::Null);
    assert_eq!(Value::from(Some(123_i32)), Value::Int(123));
    let none_val: Option<i32> = None;
    assert_eq!(Value::from(none_val), Value::Null);
}

#[test]
fn test_status_traits() {
    assert_eq!(Status::default(), Status::Stable);
    assert_eq!(Status::Stable.as_str(), "stable");
    assert_eq!(Status::Draft.as_ref(), "draft");
    assert_eq!(Status::Deprecated.to_string(), "deprecated");
    assert_eq!(Status::Other("custom".into()).as_str(), "custom");

    let parsed: Status = "draft".parse().unwrap();
    assert_eq!(parsed, Status::Draft);
    let custom_parsed: Status = "experimental".parse().unwrap();
    assert_eq!(custom_parsed, Status::Other("experimental".into()));
}

#[test]
fn test_trust_tier_traits() {
    assert_eq!(TrustTier::default(), TrustTier::Unverified);
    assert_eq!(TrustTier::Unverified.as_str(), "unverified");
    assert_eq!(TrustTier::MachineConfirmed.as_ref(), "machine-confirmed");
    assert_eq!(TrustTier::HumanReviewed.to_string(), "human-reviewed");

    assert_eq!("unverified".parse::<TrustTier>(), Ok(TrustTier::Unverified));
    assert_eq!(
        "machine-confirmed".parse::<TrustTier>(),
        Ok(TrustTier::MachineConfirmed)
    );
    assert_eq!(
        "machine_confirmed".parse::<TrustTier>(),
        Ok(TrustTier::MachineConfirmed)
    );
    assert_eq!(
        "human-reviewed".parse::<TrustTier>(),
        Ok(TrustTier::HumanReviewed)
    );
    assert_eq!(
        "human_reviewed".parse::<TrustTier>(),
        Ok(TrustTier::HumanReviewed)
    );
    assert_eq!(
        "invalid".parse::<TrustTier>(),
        Err(ParseTrustTierError("invalid".into()))
    );
}

#[test]
fn test_actor_and_actor_kind_traits() {
    let actor = Actor::from("human:walter".to_string());
    assert_eq!(actor.as_str(), "human:walter");
    assert_eq!(actor.as_ref(), "human:walter");
    assert_eq!(&*actor, "human:walter");
    assert_eq!(actor.kind(), ActorKind::Human);

    let parsed: Actor = "process:nightly".parse().unwrap();
    assert_eq!(parsed.kind(), ActorKind::Process);

    // ActorKind
    assert_eq!(ActorKind::Human.as_str(), "human");
    assert_eq!(ActorKind::Process.as_ref(), "process");
    assert_eq!(ActorKind::Agent.to_string(), "agent");
    assert_eq!("human".parse::<ActorKind>(), Ok(ActorKind::Human));
    assert_eq!("process".parse::<ActorKind>(), Ok(ActorKind::Process));
    assert_eq!("agent".parse::<ActorKind>(), Ok(ActorKind::Agent));
    assert_eq!("other".parse::<ActorKind>(), Ok(ActorKind::Other));
    assert_eq!(
        "invalid".parse::<ActorKind>(),
        Err(ParseActorKindError("invalid".into()))
    );
}

#[test]
fn test_concept_id_traits() {
    let id: ConceptId = "tables/orders".parse().unwrap();
    assert_eq!(id.as_ref(), &["tables".to_string(), "orders".to_string()]);
    assert_eq!(&*id, &["tables".to_string(), "orders".to_string()]);

    let string_id: String = id.clone().into();
    assert_eq!(string_id, "tables/orders");

    let try_from_str = ConceptId::try_from("tables/users").unwrap();
    assert_eq!(try_from_str.to_string(), "tables/users");

    let try_from_string = ConceptId::try_from("metrics/revenue".to_string()).unwrap();
    assert_eq!(try_from_string.to_string(), "metrics/revenue");
}

#[test]
fn test_link_kind_traits() {
    assert_eq!(LinkKind::Absolute.as_str(), "absolute");
    assert_eq!(LinkKind::Relative.as_ref(), "relative");
    assert_eq!(LinkKind::External.to_string(), "external");

    assert_eq!("absolute".parse::<LinkKind>(), Ok(LinkKind::Absolute));
    assert_eq!("relative".parse::<LinkKind>(), Ok(LinkKind::Relative));
    assert_eq!("external".parse::<LinkKind>(), Ok(LinkKind::External));
    assert_eq!("anchor".parse::<LinkKind>(), Ok(LinkKind::Anchor));
    assert_eq!("other".parse::<LinkKind>(), Ok(LinkKind::Other));
    assert_eq!(
        "invalid".parse::<LinkKind>(),
        Err(ParseLinkKindError("invalid".into()))
    );
}
