use crate::constants::{GENERATED_PATH_PATTERNS, MAX_TRACKED_FILE_BYTES};
use crate::language::Language;

pub fn tracked_language(path: &str) -> Option<Language> {
    if GENERATED_PATH_PATTERNS
        .iter()
        .any(|pattern| glob_matches(pattern, path))
    {
        return None;
    }
    Language::from_path(path)
}

pub fn tracked_source(bytes: &[u8]) -> Option<&str> {
    if bytes.len() > MAX_TRACKED_FILE_BYTES {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

fn glob_matches(pattern: &str, path: &str) -> bool {
    let pattern: Vec<&str> = pattern.split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    segments_match(&pattern, &path)
}

fn segments_match(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| segments_match(rest, &path[skip..])),
        Some((segment, rest)) => match path.split_first() {
            Some((name, path_rest)) => {
                segment_matches(segment, name) && segments_match(rest, path_rest)
            }
            None => false,
        },
    }
}

fn segment_matches(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    let (mut p, mut n) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while n < name.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == name[n]) {
            p += 1;
            n += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            backtrack = Some((p, n));
            p += 1;
        } else if let Some((star, matched)) = backtrack {
            p = star + 1;
            n = matched + 1;
            backtrack = Some((star, matched + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|character| *character == '*')
}

#[cfg(test)]
mod tests {
    use super::{glob_matches, tracked_language, tracked_source};
    use crate::constants::MAX_TRACKED_FILE_BYTES;
    use crate::language::Language;

    #[test]
    fn generated_and_vendored_paths_are_excluded() {
        assert!(glob_matches("**/vendor/**", "vendor/lib.rs"));
        assert!(glob_matches("**/vendor/**", "crates/app/vendor/x/lib.rs"));
        assert!(!glob_matches("**/vendor/**", "src/vendors.rs"));
        assert!(glob_matches("**/*.generated.*", "src/api.generated.ts"));
        assert!(glob_matches("**/*_pb2.py", "proto/user_pb2.py"));
        assert!(glob_matches(".marrow/**", ".marrow/db.sqlite"));
        assert!(!glob_matches(".marrow/**", "src/.marrow/db.sqlite"));
        assert_eq!(tracked_language("engine/target/debug/build.rs"), None);
        assert_eq!(tracked_language("src/main.rs"), Some(Language::Rust));
    }

    #[test]
    fn oversized_and_non_utf8_files_are_not_tracked() {
        assert_eq!(tracked_source(b"fn main() {}\n"), Some("fn main() {}\n"));
        assert_eq!(tracked_source(&[0xff, 0xfe, 0x00]), None);
        assert_eq!(
            tracked_source(&vec![b'a'; MAX_TRACKED_FILE_BYTES + 1]),
            None
        );
    }
}
