use std::collections::BTreeMap;
use std::path::Path;

use gix::ObjectId;

use crate::error::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitInfo {
    pub id: ObjectId,
    pub sha: String,
    pub authored_at: i64,
    pub parents: Vec<ObjectId>,
    pub is_merge: bool,
}

pub struct Repository {
    inner: gix::Repository,
}

impl Repository {
    pub fn open(path: &Path) -> Result<Self, Error> {
        let inner = gix::open(path).map_err(|error| Error::Open {
            path: path.display().to_string(),
            message: error.to_string(),
        })?;
        Ok(Repository { inner })
    }

    /// The commit HEAD points at.
    pub fn head(&self) -> Result<CommitInfo, Error> {
        let commit = self.inner.head_commit().map_err(git)?;
        let time = commit.time().map_err(git)?;
        let parents: Vec<ObjectId> = commit.parent_ids().map(|id| id.detach()).collect();
        Ok(CommitInfo {
            id: commit.id,
            sha: commit.id.to_string(),
            authored_at: time.seconds,
            is_merge: parents.len() > 1,
            parents,
        })
    }

    /// The first-parent history of HEAD, oldest commit first.
    pub fn first_parent_history(&self) -> Result<Vec<ObjectId>, Error> {
        let mut commit = self.inner.head_commit().map_err(git)?;
        let mut ids = vec![commit.id];
        loop {
            let Some(parent) = commit.parent_ids().next().map(|id| id.detach()) else {
                break;
            };
            commit = self.inner.find_commit(parent).map_err(git)?;
            ids.push(parent);
        }
        ids.reverse();
        Ok(ids)
    }

    /// Regular files in a commit, by path. Symlinks and submodules are left out.
    pub fn files(&self, commit: ObjectId) -> Result<BTreeMap<String, ObjectId>, Error> {
        let tree = self
            .inner
            .find_commit(commit)
            .map_err(git)?
            .tree()
            .map_err(git)?;
        let mut recorder = gix::traverse::tree::Recorder::default();
        tree.traverse().breadthfirst(&mut recorder).map_err(git)?;
        let mut files = BTreeMap::new();
        for entry in recorder.records {
            if !entry.mode.is_blob() {
                continue;
            }
            if let Ok(path) = std::str::from_utf8(entry.filepath.as_ref()) {
                files.insert(path.to_owned(), entry.oid);
            }
        }
        Ok(files)
    }

    pub fn blob(&self, id: ObjectId) -> Result<Vec<u8>, Error> {
        Ok(self.inner.find_blob(id).map_err(git)?.data.clone())
    }
}

fn git(error: impl std::fmt::Display) -> Error {
    Error::Git(error.to_string())
}
