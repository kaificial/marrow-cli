//! Turning stored path hashes back into paths. The store keeps only a hash of each path (PRD §8),
//! so a name is found by hashing every path that could be the one: the working tree, HEAD, and
//! the commits a line's history mentions.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use marrow_core::history::{CommitId, Repository};
use marrow_core::path_hash;
use marrow_core::pipeline;

use crate::capture::working_tree_files;

pub struct Paths {
    by_hash: HashMap<String, String>,
    read: HashSet<String>,
}

impl Paths {
    /// Every path in the working tree and in HEAD.
    pub fn new(root: &Path, repository: &Repository) -> Paths {
        let mut paths = Paths {
            by_hash: HashMap::new(),
            read: HashSet::new(),
        };
        for (path, _) in working_tree_files(root) {
            paths.add(path);
        }
        if let Ok(head) = repository.head() {
            paths.add_commit(repository, &head.sha);
        }
        paths
    }

    /// Every path in one commit, for files that have since been renamed or deleted.
    pub fn add_commit(&mut self, repository: &Repository, sha: &str) {
        if !self.read.insert(sha.to_owned()) {
            return;
        }
        let Ok(id) = CommitId::from_hex(sha.as_bytes()) else {
            return;
        };
        if let Ok(files) = pipeline::tracked_files(repository, id) {
            for path in files.into_keys() {
                self.add(path);
            }
        }
    }

    fn add(&mut self, path: String) {
        self.by_hash.entry(path_hash(&path)).or_insert(path);
    }

    pub fn name(&self, hash: &str) -> String {
        self.by_hash
            .get(hash)
            .cloned()
            .unwrap_or_else(|| format!("a file no longer present (path hash {hash})"))
    }
}
