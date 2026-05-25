//! `ffs://` URL parser per ADR-006.
//!
//! Grammar (informal):
//! ```text
//!   ffs://<graph>/<address>[?<query>]
//!   address := atom/<multihash>
//!            | entity/<entity-id>
//!            | <path-segments>          (path mode)
//!   query   := <pair>(&<pair>)*
//!   pair    := as_of=<iso8601> | valid_at=<iso8601> | <unknown>
//! ```
//!
//! Unknown query parameters are tolerated and ignored so the substrate
//! can extend the scheme later without breaking older clients.

use ffs_core::{EntityId, Iso8601, Multihash};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfsUrl {
    pub graph: String,
    pub address: Address,
    pub as_of: Option<Iso8601>,
    pub valid_at: Option<Iso8601>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Address {
    /// `ffs://<graph>/atom/<multihash>`
    Atom { hash: Multihash },
    /// `ffs://<graph>/entity/<entity-id>`
    Entity { id: EntityId },
    /// `ffs://<graph>/<path-segments>` — anything not matching atom/ or entity/.
    Path { path: String },
}

#[derive(Debug, Error)]
pub enum UrlError {
    #[error("missing `ffs://` scheme")]
    BadScheme,
    #[error("missing graph name after `ffs://`")]
    MissingGraph,
    #[error("missing address after graph name")]
    MissingAddress,
    #[error("invalid atom hash: {0}")]
    BadAtomHash(String),
    #[error("invalid as_of timestamp: {0}")]
    BadAsOf(String),
    #[error("invalid valid_at timestamp: {0}")]
    BadValidAt(String),
}

pub fn parse(s: &str) -> Result<FfsUrl, UrlError> {
    let after_scheme = s.strip_prefix("ffs://").ok_or(UrlError::BadScheme)?;
    let (path_part, query_part) = match after_scheme.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (after_scheme, None),
    };

    let (graph, address_part) = match path_part.split_once('/') {
        Some((g, a)) => (g, a),
        None => return Err(UrlError::MissingAddress),
    };
    if graph.is_empty() {
        return Err(UrlError::MissingGraph);
    }

    let address = parse_address(address_part)?;
    let (as_of, valid_at) = parse_query(query_part)?;

    Ok(FfsUrl {
        graph: graph.into(),
        address,
        as_of,
        valid_at,
    })
}

fn parse_address(s: &str) -> Result<Address, UrlError> {
    if let Some(rest) = s.strip_prefix("atom/") {
        let hash_str = rest.trim_end_matches('/');
        let hash = Multihash::from_multibase(hash_str)
            .map_err(|e| UrlError::BadAtomHash(e.to_string()))?;
        Ok(Address::Atom { hash })
    } else if let Some(rest) = s.strip_prefix("entity/") {
        let id_str = rest.trim_end_matches('/');
        Ok(Address::Entity {
            id: EntityId::new(id_str),
        })
    } else {
        Ok(Address::Path { path: s.into() })
    }
}

fn parse_query(q: Option<&str>) -> Result<(Option<Iso8601>, Option<Iso8601>), UrlError> {
    let mut as_of = None;
    let mut valid_at = None;
    let Some(q) = q else {
        return Ok((as_of, valid_at));
    };
    for pair in q.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        match k {
            "as_of" => {
                as_of = Some(Iso8601::new(v).map_err(|e| UrlError::BadAsOf(e.to_string()))?);
            }
            "valid_at" => {
                valid_at = Some(Iso8601::new(v).map_err(|e| UrlError::BadValidAt(e.to_string()))?);
            }
            _ => { /* tolerate unknown params */ }
        }
    }
    Ok((as_of, valid_at))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_path_url() {
        let u = parse("ffs://my-graph/contacts/by-name/S/Sarah_Chen.md").unwrap();
        assert_eq!(u.graph, "my-graph");
        assert_eq!(
            u.address,
            Address::Path {
                path: "contacts/by-name/S/Sarah_Chen.md".into()
            }
        );
        assert!(u.as_of.is_none());
    }

    #[test]
    fn parse_atom_url() {
        let h = Multihash::blake3_of(b"sample");
        let s = format!("ffs://g/atom/{}", h.to_multibase());
        let u = parse(&s).unwrap();
        assert_eq!(u.graph, "g");
        match u.address {
            Address::Atom { hash } => assert_eq!(hash, h),
            other => panic!("expected Atom, got {other:?}"),
        }
    }

    #[test]
    fn parse_entity_url() {
        let u = parse("ffs://g/entity/alice").unwrap();
        match u.address {
            Address::Entity { id } => assert_eq!(id.as_str(), "alice"),
            other => panic!("expected Entity, got {other:?}"),
        }
    }

    #[test]
    fn parse_with_as_of() {
        let u = parse("ffs://g/contacts/recent/?as_of=2026-04-15T00:00:00Z").unwrap();
        assert_eq!(u.as_of.unwrap().as_str(), "2026-04-15T00:00:00Z");
    }

    #[test]
    fn parse_with_valid_at_and_unknown_param() {
        let u = parse("ffs://g/contacts/recent/?valid_at=2026-04-15T00:00:00Z&future=x").unwrap();
        assert_eq!(u.valid_at.unwrap().as_str(), "2026-04-15T00:00:00Z");
    }

    #[test]
    fn missing_scheme_rejected() {
        assert!(matches!(
            parse("https://x/").unwrap_err(),
            UrlError::BadScheme
        ));
        assert!(matches!(
            parse("my-graph/x").unwrap_err(),
            UrlError::BadScheme
        ));
    }

    #[test]
    fn missing_graph_rejected() {
        assert!(matches!(
            parse("ffs:///contacts/").unwrap_err(),
            UrlError::MissingGraph
        ));
    }

    #[test]
    fn missing_address_rejected() {
        assert!(matches!(
            parse("ffs://my-graph").unwrap_err(),
            UrlError::MissingAddress
        ));
    }

    #[test]
    fn bad_atom_hash_rejected() {
        let err = parse("ffs://g/atom/notavalidhash").unwrap_err();
        assert!(matches!(err, UrlError::BadAtomHash(_)));
    }

    #[test]
    fn bad_as_of_rejected() {
        let err = parse("ffs://g/contacts/?as_of=not-a-date").unwrap_err();
        assert!(matches!(err, UrlError::BadAsOf(_)));
    }
}
