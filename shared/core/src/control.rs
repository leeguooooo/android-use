//! Platform-independent screen identity, delta matching and scroll geometry.
use base64::Engine as _;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

pub fn snapshot_id<T: Serialize + ?Sized>(rows: &T) -> Result<String, serde_json::Error> {
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(serde_json::to_vec(rows)?)))
}

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct ElementDelta {
    pub added: Vec<usize>,
    pub changed: Vec<usize>,
    pub removed: Vec<usize>,
    pub unchanged: usize,
}

/// Semantic matching pairs duplicate identities in document order. Indexes in
/// added/changed address the current screen; removed addresses the baseline.
pub fn diff_rows<'a, T: PartialEq, K: Eq + Hash>(
    baseline: &'a [T],
    current: &'a [T],
    identity: impl Fn(&'a T) -> K,
) -> ElementDelta {
    let mut by_identity: HashMap<K, VecDeque<usize>> = HashMap::new();
    for (index, row) in baseline.iter().enumerate() {
        by_identity
            .entry(identity(row))
            .or_default()
            .push_back(index);
    }
    let mut delta = ElementDelta::default();
    let mut matched = vec![false; baseline.len()];
    for (index, row) in current.iter().enumerate() {
        match by_identity
            .get_mut(&identity(row))
            .and_then(VecDeque::pop_front)
        {
            Some(old) => {
                matched[old] = true;
                if baseline[old] == *row {
                    delta.unchanged += 1;
                } else {
                    delta.changed.push(index);
                }
            }
            None => delta.added.push(index),
        }
    }
    delta.removed = matched
        .iter()
        .enumerate()
        .filter_map(|(i, matched)| (!matched).then_some(i))
        .collect();
    delta
}

pub fn swipe_travel(delta: f64, axis: f64) -> f64 {
    if delta == 0.0 {
        0.0
    } else {
        (delta.abs() * 1.5).clamp(0.15 * axis, 0.75 * axis) * delta.signum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn insertion_does_not_mark_following_rows_changed() {
        let delta = diff_rows(
            &[("a", 1), ("b", 2)],
            &[("x", 0), ("a", 1), ("b", 3)],
            |r| r.0,
        );
        assert_eq!(
            delta,
            ElementDelta {
                added: vec![0],
                changed: vec![2],
                removed: vec![],
                unchanged: 1
            }
        );
    }
    #[test]
    fn duplicates_pair_in_document_order() {
        let delta = diff_rows(&[("a", 1), ("a", 2)], &[("a", 2)], |r| r.0);
        assert_eq!((delta.changed, delta.removed), (vec![0], vec![1]));
    }
}
