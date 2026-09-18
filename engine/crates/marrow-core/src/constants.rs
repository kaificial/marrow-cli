//! Every tunable number from docs/specs/genealogy.md, named exactly as in its constants table.

pub const MAX_TRACKED_FILE_BYTES: usize = 1_048_576;

pub const PARSE_TIMEOUT_MICROS: u64 = 10_000_000;

pub const GENERATED_PATH_PATTERNS: &[&str] = &[
    "**/vendor/**",
    "**/third_party/**",
    "**/node_modules/**",
    "**/target/**",
    "**/dist/**",
    "**/build/**",
    "**/__generated__/**",
    "**/*.generated.*",
    "**/*.min.js",
    "**/*_pb2.py",
    "**/*_pb2_grpc.py",
    ".marrow/**",
];

pub const FILE_RENAME_MIN_JACCARD: f64 = 0.5;

pub const HISTOGRAM_MAX_OCCURRENCES: usize = 64;

pub const WEAK_ANCHOR_MAX_CONTENT_TOKENS: u32 = 2;

pub const REFLOW_MAX_GROUP_LINES: usize = 20;

pub const PREFILTER_MIN_UNIGRAM_JACCARD: f64 = 0.2;

pub const EDIT_MIN_DICE: f64 = 0.5;

pub const HUNK_FULL_ALIGNMENT_MAX_CELLS: usize = 250_000;

pub const HUNK_ALIGNMENT_BAND_LINES: usize = 100;

pub const REWRITE_MAX_RETENTION: f64 = 0.4;

pub const MIN_MOVE_BLOCK_LINES: usize = 3;

pub const MIN_MOVE_BLOCK_CONTENT_TOKENS: u32 = 8;

pub const SINGLE_LINE_MOVE_MIN_CONTENT_TOKENS: u32 = 2;

pub const MOVE_SEED_MAX_CANDIDATES: usize = 64;
