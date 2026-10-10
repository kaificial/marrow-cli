use std::collections::BTreeMap;
use std::path::Path;

use gix::ObjectId;

use crate::content::ContentId;
use crate::error::Error;

/// A commit's id, for callers that walk history without depending on git internals.
pub type CommitId = ObjectId;

/// The longest agent trailer kept. A trailer names a tool and a model; anything longer is noise.
const MAX_TRAILER_CHARS: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitInfo {
    pub id: ObjectId,
    pub sha: String,
    /// When the change was written, which survives a rebase or cherry-pick unchanged.
    pub authored_at: i64,
    /// When this commit object was made, which is when its tree appeared on the branch. Along
    /// first-parent history this is the clock that runs forwards, and it's the one `git log
    /// --since` filters on, so it's what observations are dated by.
    pub committed_at: i64,
    pub parents: Vec<ObjectId>,
    pub is_merge: bool,
    /// A sign in the message that Claude Code helped write this commit. It says an agent was
    /// involved, not which lines it wrote, so it is kept beside a line's origin, never in place
    /// of it.
    pub agent_trailer: Option<String>,
    /// The first line of the message, for showing a person which commit this is. Never stored.
    pub subject: String,
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
        let id = self.inner.head_commit().map_err(git)?.id;
        self.commit(id)
    }

    pub fn commit(&self, id: ObjectId) -> Result<CommitInfo, Error> {
        let commit = self.inner.find_commit(id).map_err(git)?;
        let authored_at = commit.author().map_err(git)?.time.seconds;
        let committed_at = commit.committer().map_err(git)?.time.seconds;
        let parents: Vec<ObjectId> = commit.parent_ids().map(|parent| parent.detach()).collect();
        let message = String::from_utf8_lossy(commit.message_raw().map_err(git)?).into_owned();
        Ok(CommitInfo {
            id,
            sha: id.to_string(),
            authored_at,
            committed_at,
            is_merge: parents.len() > 1,
            parents,
            agent_trailer: agent_trailer(&message),
            subject: message.lines().next().unwrap_or_default().trim().to_owned(),
        })
    }

    /// Whether HEAD points at a commit yet. A new repository's HEAD is unborn, which isn't an
    /// error, and mustn't be confused with one.
    pub fn has_commits(&self) -> Result<bool, Error> {
        Ok(!self.inner.head().map_err(git)?.is_unborn())
    }

    /// The first-parent history of HEAD, oldest commit first.
    pub fn first_parent_history(&self) -> Result<Vec<ObjectId>, Error> {
        Ok(self.first_parent_walk()?.0)
    }

    /// The first-parent history of HEAD, oldest commit first, and whether it is cut short. In a
    /// shallow clone the walk stops at the boundary git recorded, and the oldest commit it
    /// reaches isn't the repository's first: its lines were born earlier.
    pub fn first_parent_walk(&self) -> Result<(Vec<ObjectId>, bool), Error> {
        let boundary: Vec<ObjectId> = self
            .inner
            .shallow_commits()
            .map_err(git)?
            .map(|commits| commits.iter().copied().collect())
            .unwrap_or_default();
        let mut commit = self.inner.head_commit().map_err(git)?;
        let mut ids = vec![commit.id];
        let mut truncated = false;
        loop {
            if boundary.contains(&commit.id) {
                truncated = true;
                break;
            }
            let Some(parent) = commit.parent_ids().next().map(|id| id.detach()) else {
                break;
            };
            commit = self.inner.find_commit(parent).map_err(git)?;
            ids.push(parent);
        }
        ids.reverse();
        Ok((ids, truncated))
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

    /// A file's contents at a commit, or None if the commit doesn't have it.
    pub fn file_at(&self, commit: ObjectId, path: &str) -> Result<Option<Vec<u8>>, Error> {
        match self.files(commit)?.get(path) {
            Some(id) => Ok(Some(self.blob(*id)?)),
            None => Ok(None),
        }
    }

    /// How big a file's contents are, read from the object header without inflating the blob.
    pub fn blob_size(&self, content: ContentId) -> Result<u64, Error> {
        let id = ObjectId::try_from(&content.bytes()[..]).map_err(git)?;
        Ok(self.inner.find_header(id).map_err(git)?.size())
    }
}

/// Finds Claude Code's mark in a commit message: a `Co-Authored-By` trailer with Claude
/// Code's own address, or a line that is Claude Code's "Generated with Claude Code" footer.
/// The trailer is preferred because it names the model.
///
/// Both are matched narrowly on purpose. A co-author called Claude, a person who works at
/// Anthropic, or a message that merely mentions Claude Code is not a sign the agent was
/// involved, and the trailer's text is stored, so only Claude Code's own trailer is.
pub fn agent_trailer(message: &str) -> Option<String> {
    const KEY: &str = "co-authored-by:";
    const ADDRESS: &str = "<noreply@anthropic.com>";
    let mut footer = None;
    for line in message.lines().map(str::trim) {
        let key = line
            .get(..KEY.len())
            .filter(|key| key.eq_ignore_ascii_case(KEY));
        if key.is_some() {
            let value = line[KEY.len()..].trim();
            if value.to_lowercase().ends_with(ADDRESS) {
                return Some(value.chars().take(MAX_TRAILER_CHARS).collect());
            }
            continue;
        }
        // The footer is a line of its own, usually after an emoji.
        let text = line
            .trim_start_matches(|character: char| !character.is_alphanumeric())
            .to_lowercase();
        if text.starts_with("generated with [claude code]")
            || text.starts_with("generated with claude code")
        {
            footer = Some("Generated with Claude Code".to_owned());
        }
    }
    footer
}

fn git(error: impl std::fmt::Display) -> Error {
    Error::Git(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::agent_trailer;

    #[test]
    fn claude_code_trailers_are_found_and_name_the_model() {
        assert_eq!(
            agent_trailer("fix: tidy\n\nCo-Authored-By: Claude <noreply@anthropic.com>\n")
                .as_deref(),
            Some("Claude <noreply@anthropic.com>")
        );
        assert_eq!(
            agent_trailer("feat: x\n\nco-authored-by: Claude Opus 5 <noreply@anthropic.com>")
                .as_deref(),
            Some("Claude Opus 5 <noreply@anthropic.com>")
        );
        assert_eq!(
            agent_trailer(
                "feat: x\n\n🤖 Generated with [Claude Code](https://claude.com/claude-code)\n"
            )
            .as_deref(),
            Some("Generated with Claude Code")
        );
    }

    #[test]
    fn the_trailer_wins_over_the_footer_because_it_names_the_model() {
        let message = "feat: x\n\n🤖 Generated with [Claude Code](https://claude.com/claude-code)\n\nCo-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>\n";
        assert_eq!(
            agent_trailer(message).as_deref(),
            Some("Claude Sonnet 5 <noreply@anthropic.com>")
        );
    }

    #[test]
    fn people_and_passing_mentions_are_not_agent_trailers() {
        assert_eq!(
            agent_trailer("feat: x\n\nCo-authored-by: Jane Doe <jane@example.com>\n"),
            None
        );
        assert_eq!(
            agent_trailer("feat: x\n\nCo-authored-by: Claude Shannon <claude@bell-labs.com>\n"),
            None,
            "a person named Claude"
        );
        assert_eq!(
            agent_trailer("feat: x\n\nCo-authored-by: Sam Person <sam@anthropic.com>\n"),
            None,
            "a person who works at Anthropic"
        );
        assert_eq!(
            agent_trailer("docs: note that this was generated with claude code, then rewritten\n"),
            None,
            "a sentence that mentions the footer's words"
        );
        assert_eq!(
            agent_trailer("docs: explain how Claude Code hooks call marrow record\n"),
            None
        );
        assert_eq!(agent_trailer(""), None);
    }
}
