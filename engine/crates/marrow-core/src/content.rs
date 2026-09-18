//! Content identity: git's own blob id, so a file captured from a write and the same file read
//! out of a commit are recognisably the same bytes.

/// A git blob id. Comparing two of these answers "is this the same file contents?" without
/// keeping the contents.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ContentId([u8; 20]);

impl ContentId {
    /// The id no real blob has. A stored snapshot that never recorded its content gets this, so
    /// it never compares equal to a committed file and is always re-examined.
    pub const UNKNOWN: ContentId = ContentId([0; 20]);

    /// The id git would give a blob holding these bytes.
    pub fn of(bytes: &[u8]) -> ContentId {
        let id = gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, bytes);
        ContentId(id.as_bytes().try_into().expect("a sha-1 is twenty bytes"))
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<ContentId> {
        bytes.try_into().ok().map(ContentId)
    }

    pub fn from_hex(text: &str) -> Option<ContentId> {
        gix::ObjectId::from_hex(text.as_bytes())
            .ok()
            .and_then(|id| ContentId::from_bytes(id.as_bytes()))
    }

    /// Whether this is a real blob id. Two unknown ids must never count as the same contents,
    /// so every comparison that concludes "unchanged" has to ask first.
    pub fn is_known(self) -> bool {
        self != ContentId::UNKNOWN
    }

    pub fn to_hex(self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::ContentId;

    #[test]
    fn the_id_is_the_one_git_would_give() {
        // `git hash-object` on a file holding "hello\n".
        assert_eq!(
            ContentId::of(b"hello\n").to_hex(),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        );
        assert_eq!(
            ContentId::of(b"").to_hex(),
            "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"
        );
    }

    #[test]
    fn hex_round_trips_and_nonsense_is_rejected() {
        let id = ContentId::of(b"fn main() {}\n");
        assert_eq!(ContentId::from_hex(&id.to_hex()), Some(id));
        assert_eq!(ContentId::from_hex("not-a-hash"), None);
        assert_ne!(id, ContentId::UNKNOWN);
        assert!(id.is_known());
        assert!(!ContentId::UNKNOWN.is_known());
        assert!(!ContentId::default().is_known());
    }
}
