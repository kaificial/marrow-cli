from . import (
    delete_function,
    duplicate_heavy,
    format_only,
    full_rewrite,
    move_across_files,
    move_within_file,
    reindent,
    rename_file,
    rename_identifier,
)

ALL_FIXTURES = [
    *format_only.FIXTURES,
    *rename_identifier.FIXTURES,
    *rename_file.FIXTURES,
    *move_within_file.FIXTURES,
    *move_across_files.FIXTURES,
    *delete_function.FIXTURES,
    *reindent.FIXTURES,
    *full_rewrite.FIXTURES,
    *duplicate_heavy.FIXTURES,
]
