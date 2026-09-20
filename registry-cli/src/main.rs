//! `registry` — the generic CLI over `org.icedtea.Registry`.
//!
//! Reads the type tag on the wire, so it can show and set keys it has no
//! compiled schema for. Commands: `get`, `get-stored`, `set`, `unset`,
//! `reset`, `list`, `dump`, `spec`, `seq`, `watch`, `load`.

use std::collections::BTreeMap;
use std::process::ExitCode;

use icedtea_registry::{Registry, RegistryError, Value};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("registry: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), RegistryError> {
    let Some(command) = args.first().map(String::as_str) else {
        usage();
        return Ok(());
    };
    let registry = Registry::connect()?;
    match command {
        "get" => {
            let path = arg(args, 1)?;
            let (value, is_default) = registry.get(path)?;
            print_json(&value)?;
            println!("source: {}", if is_default { "default" } else { "stored" });
        }
        "get-stored" => {
            let path = arg(args, 1)?;
            match registry.get_stored(path)? {
                Some(value) => print_json(&value)?,
                None => println!("(unset)"),
            }
        }
        "set" => {
            let path = arg(args, 1)?;
            let raw = arg(args, 2)?;
            let value = parse_value(raw)?;
            let seq = registry.set(path, &value)?;
            println!("{seq}");
        }
        "unset" => {
            let path = arg(args, 1)?;
            println!("{}", registry.unset(path)?);
        }
        "reset" => {
            let prefix = arg(args, 1)?;
            println!("{}", registry.reset(prefix)?);
        }
        "list" => {
            let prefix = arg(args, 1)?;
            let recurse = args.iter().any(|a| a == "--recurse");
            for (path, tag) in registry.list(prefix, recurse)? {
                println!("{path}\t{}", tag.name());
            }
        }
        "dump" => {
            let prefix = arg(args, 1)?;
            let rows = registry.dump(prefix)?;
            let mut map = BTreeMap::new();
            for (path, value) in rows {
                map.insert(path, value);
            }
            print_json(&serde_json::to_value(map).unwrap_or(serde_json::Value::Null))?;
        }
        "spec" => {
            let path = arg(args, 1)?;
            match registry.spec_json(path)? {
                Some(json) => println!("{json}"),
                None => {
                    println!("(unregistered)");
                }
            }
        }
        "seq" => println!("{}", registry.seq()?),
        "watch" => {
            let prefix = arg(args, 1)?.to_string();
            println!("watching {prefix} (ctrl-c to stop)");
            registry.watch(prefix, |changes, seq| {
                for (path, present) in changes {
                    println!(
                        "{seq}\t{}\t{}",
                        if *present { "set" } else { "unset" },
                        path
                    );
                }
            })?;
            loop {
                std::thread::park();
            }
        }
        "scan" => {
            let summary = icedtea_registry_schema::scan::ingest(
                &registry,
                &icedtea_registry_schema::scan::default_app_dirs(),
                &icedtea_registry_schema::scan::default_autostart_dirs(),
            )?;
            println!(
                "apps: {}\thandler lists changed: {}\tautostart: {}\tunchanged: {}",
                summary.apps, summary.handler_lists_changed, summary.autostart, summary.unchanged
            );
        }
        "load" => {
            let file = arg(args, 1)?;
            let text = std::fs::read_to_string(file).map_err(io_err)?;
            let parsed: BTreeMap<String, Value> = serde_json::from_str(&text)
                .map_err(|e| RegistryError::Storage(format!("load file is not valid: {e}")))?;
            let changes: Vec<(String, Value)> = parsed.into_iter().collect();
            println!("{}", registry.set_many(&changes)?);
        }
        other => {
            eprintln!("registry: unknown command {other:?}");
            usage();
            return Err(RegistryError::Unavailable("unknown command".into()));
        }
    }
    Ok(())
}

fn usage() {
    eprintln!(
        "usage: registry <command> [args]\n\
         \n\
         get <path>            effective value (stored, else default)\n\
         get-stored <path>     stored value only\n\
         set <path> <value>    set (value: JSON, or bare string/number/bool)\n\
         unset <path>          revert one key to its default\n\
         reset <prefix>        revert a subtree\n\
         list <prefix> [--recurse]\n\
         dump <prefix>         paths and values as JSON\n\
         spec <path>           a registered key's spec as JSON\n\
         seq                   current commit sequence\n\
         watch <prefix>        print changes until interrupted\n\
         scan                  ingest .desktop/autostart/globs2 registrations\n\
         load <file>           set many from a JSON object of path -> value"
    );
}

fn arg(args: &[String], index: usize) -> Result<&str, RegistryError> {
    args.get(index)
        .map(String::as_str)
        .ok_or_else(|| RegistryError::Unavailable("missing argument".into()))
}

/// Accept the tagged JSON form, or a bare `true`/`false`/integer/string.
fn parse_value(raw: &str) -> Result<Value, RegistryError> {
    if let Ok(value) = serde_json::from_str::<Value>(raw)
        && raw.trim_start().starts_with('{')
    {
        return Ok(value);
    }
    if raw == "true" {
        return Ok(Value::Bool(true));
    }
    if raw == "false" {
        return Ok(Value::Bool(false));
    }
    if let Ok(n) = raw.parse::<i64>() {
        return Ok(if n < 0 {
            Value::Int(n)
        } else {
            Value::Uint(n as u64)
        });
    }
    Ok(Value::Str(raw.to_string()))
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<(), RegistryError> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| RegistryError::Storage(format!("could not render JSON: {e}")))?;
    println!("{text}");
    Ok(())
}

fn io_err(err: std::io::Error) -> RegistryError {
    RegistryError::Storage(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_values_parse() {
        assert_eq!(parse_value("42").unwrap(), Value::Uint(42));
        assert_eq!(parse_value("-3").unwrap(), Value::Int(-3));
        assert_eq!(parse_value("true").unwrap(), Value::Bool(true));
        assert_eq!(parse_value("hello").unwrap(), Value::Str("hello".into()));
    }

    #[test]
    fn tagged_json_parses() {
        assert_eq!(
            parse_value(r#"{"type":"uint","value":7}"#).unwrap(),
            Value::Uint(7)
        );
    }
}
