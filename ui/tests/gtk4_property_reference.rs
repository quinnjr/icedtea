//! Reference conformance: the property registry is GTK 4.22's CSS property
//! table, not a subset of it and not a superset.
//!
//! The fixture is vendored text rather than a live fetch so the gate is
//! hermetic and reviewable in a diff.

use icedtea_ui::css::registry::{N_LONGHANDS, N_PROPS, PROPERTIES, PropertyKind, lookup};

const REFERENCE: &str = include_str!("fixtures/gtk4.22-css-properties.txt");

#[derive(Debug, PartialEq, Eq)]
struct Row {
    name: String,
    longhand: bool,
    inherited: bool,
}

/// `name|L|yes` / `name|S|no`; `#` comments and blank lines are skipped.
/// A malformed row is a test failure, never a silently dropped line.
fn reference_rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for (number, line) in REFERENCE.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split('|').collect();
        assert_eq!(
            fields.len(),
            3,
            "fixture line {} is not `name|kind|inherited`: {line:?}",
            number + 1
        );
        let longhand = match fields[1] {
            "L" => true,
            "S" => false,
            other => panic!("fixture line {} has kind {other:?}, not L or S", number + 1),
        };
        let inherited = match fields[2] {
            "yes" => true,
            "no" => false,
            other => panic!(
                "fixture line {} has inherited {other:?}, not yes or no",
                number + 1
            ),
        };
        rows.push(Row {
            name: fields[0].to_string(),
            longhand,
            inherited,
        });
    }
    rows
}

#[test]
fn every_gtk4_property_is_registered() {
    let rows = reference_rows();
    let mut missing = Vec::new();
    let mut wrong_kind = Vec::new();
    let mut wrong_inheritance = Vec::new();

    for row in &rows {
        let Some(prop) = lookup(&row.name) else {
            missing.push(row.name.clone());
            continue;
        };
        if prop.is_longhand() != row.longhand {
            wrong_kind.push(format!(
                "{}: registry says {}, GTK 4.22 says {}",
                row.name,
                if prop.is_longhand() {
                    "longhand"
                } else {
                    "shorthand"
                },
                if row.longhand {
                    "longhand"
                } else {
                    "shorthand"
                },
            ));
        }
        if prop.is_inherited() != row.inherited {
            wrong_inheritance.push(format!(
                "{}: registry says inherited={}, GTK 4.22 says {}",
                row.name,
                prop.is_inherited(),
                row.inherited
            ));
        }
    }

    assert!(
        missing.is_empty(),
        "{} GTK 4.22 properties are not in the registry: {missing:?}",
        missing.len()
    );
    assert!(
        wrong_kind.is_empty(),
        "wrong longhand/shorthand kind: {wrong_kind:?}"
    );
    assert!(
        wrong_inheritance.is_empty(),
        "wrong inherited flag: {wrong_inheritance:?}"
    );
}

#[test]
fn the_registry_holds_nothing_gtk_does_not() {
    let rows = reference_rows();
    let extra: Vec<&str> = PROPERTIES
        .iter()
        .map(|def| def.name)
        .filter(|name| !rows.iter().any(|row| row.name == *name))
        .collect();
    assert!(
        extra.is_empty(),
        "the registry invents {} properties GTK 4.22 does not have: {extra:?}",
        extra.len()
    );
}

#[test]
fn the_registry_row_order_is_the_reference_order() {
    let rows = reference_rows();
    assert_eq!(
        rows.len(),
        N_PROPS,
        "the fixture has {} rows, the registry has {N_PROPS}",
        rows.len()
    );
    assert_eq!(
        rows.iter().filter(|row| row.longhand).count(),
        N_LONGHANDS,
        "longhand count disagrees with N_LONGHANDS"
    );
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(
            PROPERTIES[index].name, row.name,
            "registry slot {index} is {:?}, the reference says {:?}",
            PROPERTIES[index].name, row.name
        );
    }
    // Longhands occupy 0..N_LONGHANDS, shorthands the rest: the ComputedStyle
    // slot rule depends on it.
    for (index, def) in PROPERTIES.iter().enumerate() {
        let is_longhand = matches!(def.kind, PropertyKind::Longhand { .. });
        assert_eq!(
            is_longhand,
            index < N_LONGHANDS,
            "{} sits at slot {index}, on the wrong side of N_LONGHANDS",
            def.name
        );
    }
}

#[test]
fn property_lookup_is_ascii_case_insensitive() {
    for row in reference_rows() {
        let upper = row.name.to_ascii_uppercase();
        assert_eq!(
            lookup(&upper),
            lookup(&row.name),
            "{upper} and {} resolve differently",
            row.name
        );
    }
    assert_eq!(lookup("nosuchproperty"), None);
    assert_eq!(lookup(""), None);
    assert_eq!(lookup("-gtk-"), None);
}
