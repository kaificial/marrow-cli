use std::cmp::Ordering;

use crate::constants::FILE_RENAME_MIN_JACCARD;
use crate::pipeline::{Snapshot, TrackedFile};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pairing {
    SamePath,
    ExactRename,
    SimilarRename,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FilePair {
    pub old_path: String,
    pub new_path: String,
    pub pairing: Pairing,
    pub similarity: f64,
}

#[derive(Debug, Default, PartialEq)]
pub struct Correspondence {
    pub pairs: Vec<FilePair>,
    pub deleted: Vec<String>,
    pub added: Vec<String>,
}

/// Layer 1: decides which old and new files are the same file.
pub fn correspond(previous: &Snapshot, current: &Snapshot) -> Correspondence {
    let mut pairs = Vec::new();
    let mut deleted = Vec::new();
    for (path, old) in previous {
        match current.get(path) {
            Some(new) => pairs.push(FilePair {
                old_path: path.clone(),
                new_path: path.clone(),
                pairing: Pairing::SamePath,
                similarity: jaccard(&fingerprint_set(old), &fingerprint_set(new)),
            }),
            None => deleted.push(path.clone()),
        }
    }
    let mut added: Vec<String> = current
        .keys()
        .filter(|path| !previous.contains_key(*path))
        .cloned()
        .collect();

    let exact = candidates(&deleted, &added, |old, new| {
        let (old, new) = (&previous[&deleted[old]], &current[&added[new]]);
        (old.content.is_known() && old.content == new.content).then_some(1.0)
    });
    accept(
        exact,
        Pairing::ExactRename,
        &mut pairs,
        &mut deleted,
        &mut added,
    );

    let deleted_sets: Vec<Vec<u64>> = deleted
        .iter()
        .map(|path| fingerprint_set(&previous[path]))
        .collect();
    let added_sets: Vec<Vec<u64>> = added
        .iter()
        .map(|path| fingerprint_set(&current[path]))
        .collect();
    let similar = candidates(&deleted, &added, |old, new| {
        let similarity = jaccard(&deleted_sets[old], &added_sets[new]);
        (similarity >= FILE_RENAME_MIN_JACCARD).then_some(similarity)
    });
    accept(
        similar,
        Pairing::SimilarRename,
        &mut pairs,
        &mut deleted,
        &mut added,
    );

    Correspondence {
        pairs,
        deleted,
        added,
    }
}

struct Candidate {
    old_path: String,
    new_path: String,
    similarity: f64,
}

fn candidates(
    deleted: &[String],
    added: &[String],
    mut score: impl FnMut(usize, usize) -> Option<f64>,
) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for (old_index, old_path) in deleted.iter().enumerate() {
        for (new_index, new_path) in added.iter().enumerate() {
            if let Some(similarity) = score(old_index, new_index) {
                candidates.push(Candidate {
                    old_path: old_path.clone(),
                    new_path: new_path.clone(),
                    similarity,
                });
            }
        }
    }
    candidates.sort_by(|a, b| {
        b.similarity
            .total_cmp(&a.similarity)
            .then_with(|| same(file_name, b).cmp(&same(file_name, a)))
            .then_with(|| same(parent, b).cmp(&same(parent, a)))
            .then_with(|| a.old_path.cmp(&b.old_path))
            .then_with(|| a.new_path.cmp(&b.new_path))
    });
    candidates
}

fn accept(
    candidates: Vec<Candidate>,
    pairing: Pairing,
    pairs: &mut Vec<FilePair>,
    deleted: &mut Vec<String>,
    added: &mut Vec<String>,
) {
    for candidate in candidates {
        let (Some(old_index), Some(new_index)) = (
            deleted.iter().position(|path| *path == candidate.old_path),
            added.iter().position(|path| *path == candidate.new_path),
        ) else {
            continue;
        };
        deleted.remove(old_index);
        added.remove(new_index);
        pairs.push(FilePair {
            old_path: candidate.old_path,
            new_path: candidate.new_path,
            pairing,
            similarity: candidate.similarity,
        });
    }
}

fn same(part: fn(&str) -> &str, candidate: &Candidate) -> bool {
    part(&candidate.old_path) == part(&candidate.new_path)
}

fn file_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn fingerprint_set(file: &TrackedFile) -> Vec<u64> {
    let mut set: Vec<u64> = file
        .lines
        .iter()
        .filter(|line| line.content_tokens > 0)
        .map(|line| line.fingerprint)
        .collect();
    set.sort_unstable();
    set.dedup();
    set
}

/// Jaccard similarity of two sorted, deduplicated sets.
pub(crate) fn jaccard(a: &[u64], b: &[u64]) -> f64 {
    let (mut i, mut j, mut shared) = (0, 0, 0usize);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            Ordering::Less => i += 1,
            Ordering::Greater => j += 1,
            Ordering::Equal => {
                shared += 1;
                i += 1;
                j += 1;
            }
        }
    }
    let union = a.len() + b.len() - shared;
    if union == 0 {
        0.0
    } else {
        shared as f64 / union as f64
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{correspond, jaccard, Pairing};
    use crate::content::ContentId;
    use crate::language::Language;
    use crate::normalize::normalize;
    use crate::pipeline::{Snapshot, TrackedFile};

    fn file(id: u8, source: &str) -> TrackedFile {
        TrackedFile {
            content: ContentId::from_bytes(&[id; 20]).expect("twenty bytes"),
            lines: Arc::new(normalize(source, Language::Rust).expect("parses")),
        }
    }

    fn snapshot(files: Vec<(&str, TrackedFile)>) -> Snapshot {
        files
            .into_iter()
            .map(|(path, file)| (path.to_owned(), file))
            .collect()
    }

    const PARSER: &str = "//! Rows.\npub struct Item {\n    pub name: String,\n}\npub fn parse(line: &str) -> Item {\n    let name = line.trim().to_string();\n    Item { name }\n}\n";
    const EDITED_PARSER: &str = "//! Stock rows.\npub struct Item {\n    pub name: String,\n}\npub fn parse(line: &str) -> Item {\n    let name = line.trim().to_string();\n    Item { name }\n}\n";
    const LEGACY: &str = "fn main() {\n    let raw = std::io::read_to_string(std::io::stdin()).unwrap();\n    println!(\"{}\", raw.len());\n}\n";
    const REPORT: &str =
        "fn main() {\n    for arg in std::env::args() {\n        println!(\"{arg}\");\n    }\n}\n";

    #[test]
    fn same_paths_are_always_paired() {
        let result = correspond(
            &snapshot(vec![("src/a.rs", file(1, PARSER))]),
            &snapshot(vec![("src/a.rs", file(2, LEGACY))]),
        );
        assert_eq!(result.pairs.len(), 1);
        assert_eq!(result.pairs[0].pairing, Pairing::SamePath);
        assert!(result.deleted.is_empty() && result.added.is_empty());
    }

    #[test]
    fn exact_and_similar_renames_are_paired_but_unrelated_files_are_not() {
        let result = correspond(
            &snapshot(vec![
                ("src/inventory.rs", file(1, PARSER)),
                ("src/bin/legacy.rs", file(2, LEGACY)),
                ("src/copy_me.rs", file(3, REPORT)),
            ]),
            &snapshot(vec![
                ("src/stock.rs", file(4, EDITED_PARSER)),
                ("src/bin/report.rs", file(5, REPORT)),
                ("src/moved/copy_me.rs", file(3, REPORT)),
            ]),
        );
        let found: Vec<(&str, &str, Pairing)> = result
            .pairs
            .iter()
            .map(|pair| (pair.old_path.as_str(), pair.new_path.as_str(), pair.pairing))
            .collect();
        assert_eq!(
            found,
            [
                (
                    "src/copy_me.rs",
                    "src/moved/copy_me.rs",
                    Pairing::ExactRename
                ),
                ("src/inventory.rs", "src/stock.rs", Pairing::SimilarRename),
            ]
        );
        assert_eq!(result.deleted, ["src/bin/legacy.rs"]);
        assert_eq!(result.added, ["src/bin/report.rs"]);
    }

    #[test]
    fn ties_go_to_the_same_file_name() {
        let result = correspond(
            &snapshot(vec![("a/util.rs", file(1, PARSER))]),
            &snapshot(vec![
                ("b/other.rs", file(1, PARSER)),
                ("c/util.rs", file(1, PARSER)),
            ]),
        );
        assert_eq!(result.pairs[0].new_path, "c/util.rs");
        assert_eq!(result.added, ["b/other.rs"]);
    }

    #[test]
    fn lines_without_content_tokens_do_not_count() {
        let braces = file(1, "fn a() {\n}\n");
        let other = file(2, "fn b() {\n}\n");
        let result = correspond(
            &snapshot(vec![("x.rs", braces)]),
            &snapshot(vec![("y.rs", other)]),
        );
        assert!(result.pairs.is_empty());
        assert_eq!(jaccard(&[], &[]), 0.0);
        assert_eq!(jaccard(&[1, 2, 3], &[2, 3, 4]), 0.5);
    }
}
