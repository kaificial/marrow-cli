from fixture_lib import Commit, Fixture

RUST = Fixture(
    mutation_class="duplicate-heavy",
    language="rust",
    summary=(
        "validate has six guard blocks with an identical `return Err(String::from(\"invalid item\"));` "
        "line and an identical closing `    }`; two of them also log with an identical "
        "`log_rejection(item);`. Only the third block is edited: a born `log_rejection(item);` (identical "
        "to the two existing ones) is inserted, and its return message is edited in place. Every later "
        "line shifts down by one. A matcher that pairs identical lines as a set, or by line number, "
        "mislabels the duplicates."
    ),
    commits=[
        Commit(
            message="add item validation",
            files={
                "src/validate.rs": r"""
u1  + | use crate::inventory::Item;
      |
l1  + | fn log_rejection(item: &Item) {
l2  + |     eprintln!("rejected {}", item.name);
l3  + | }
      |
fn1 + | pub fn validate(item: &Item) -> Result<(), String> {
ga1 + |     if item.name.is_empty() {
ga2 + |         return Err(String::from("invalid item"));
ga3 + |     }
gb1 + |     if item.name.len() > 64 {
gb2 + |         log_rejection(item);
gb3 + |         return Err(String::from("invalid item"));
gb4 + |     }
gc1 + |     if item.quantity == 0 {
gc2 + |         return Err(String::from("invalid item"));
gc3 + |     }
gd1 + |     if item.quantity > 10_000 {
gd2 + |         return Err(String::from("invalid item"));
gd3 + |     }
ge1 + |     if item.price_cents == 0 {
ge2 + |         log_rejection(item);
ge3 + |         return Err(String::from("invalid item"));
ge4 + |     }
gf1 + |     if item.price_cents > 1_000_000 {
gf2 + |         return Err(String::from("invalid item"));
gf3 + |     }
fn2 + |     Ok(())
fn3 + | }
""",
            },
        ),
        Commit(
            message="explain zero-quantity rejections",
            files={
                "src/validate.rs": r"""
u1  = | use crate::inventory::Item;
      |
l1  = | fn log_rejection(item: &Item) {
l2  = |     eprintln!("rejected {}", item.name);
l3  = | }
      |
fn1 = | pub fn validate(item: &Item) -> Result<(), String> {
ga1 = |     if item.name.is_empty() {
ga2 = |         return Err(String::from("invalid item"));
ga3 = |     }
gb1 = |     if item.name.len() > 64 {
gb2 = |         log_rejection(item);
gb3 = |         return Err(String::from("invalid item"));
gb4 = |     }
gc1 = |     if item.quantity == 0 {
n1  + |         log_rejection(item);
gc2 ~ |         return Err(String::from("quantity must be positive"));
gc3 = |     }
gd1 = |     if item.quantity > 10_000 {
gd2 = |         return Err(String::from("invalid item"));
gd3 = |     }
ge1 = |     if item.price_cents == 0 {
ge2 = |         log_rejection(item);
ge3 = |         return Err(String::from("invalid item"));
ge4 = |     }
gf1 = |     if item.price_cents > 1_000_000 {
gf2 = |         return Err(String::from("invalid item"));
gf3 = |     }
fn2 = |     Ok(())
fn3 = | }
""",
            },
        ),
    ],
)

PYTHON = Fixture(
    mutation_class="duplicate-heavy",
    language="python",
    summary=(
        "validate has six guard blocks with an identical `raise ValueError(\"invalid item\")` line; two of "
        "them also log with an identical `log_rejection(item)`. Only the third block is edited: a born "
        "`log_rejection(item)` (identical to the two existing ones) is inserted, and its raise message is "
        "edited in place. Every later line shifts down by one. A matcher that pairs identical lines as a "
        "set, or by line number, mislabels the duplicates."
    ),
    commits=[
        Commit(
            message="add item validation",
            files={
                "validate.py": r"""
u1  + | from inventory import Item
      |
      |
l1  + | def log_rejection(item: Item) -> None:
l2  + |     print(f"rejected {item.name}")
      |
      |
fn1 + | def validate(item: Item) -> None:
ga1 + |     if not item.name:
ga2 + |         raise ValueError("invalid item")
gb1 + |     if len(item.name) > 64:
gb2 + |         log_rejection(item)
gb3 + |         raise ValueError("invalid item")
gc1 + |     if item.quantity == 0:
gc2 + |         raise ValueError("invalid item")
gd1 + |     if item.quantity > 10_000:
gd2 + |         raise ValueError("invalid item")
ge1 + |     if item.price_cents == 0:
ge2 + |         log_rejection(item)
ge3 + |         raise ValueError("invalid item")
gf1 + |     if item.price_cents > 1_000_000:
gf2 + |         raise ValueError("invalid item")
""",
            },
        ),
        Commit(
            message="explain zero-quantity rejections",
            files={
                "validate.py": r"""
u1  = | from inventory import Item
      |
      |
l1  = | def log_rejection(item: Item) -> None:
l2  = |     print(f"rejected {item.name}")
      |
      |
fn1 = | def validate(item: Item) -> None:
ga1 = |     if not item.name:
ga2 = |         raise ValueError("invalid item")
gb1 = |     if len(item.name) > 64:
gb2 = |         log_rejection(item)
gb3 = |         raise ValueError("invalid item")
gc1 = |     if item.quantity == 0:
n1  + |         log_rejection(item)
gc2 ~ |         raise ValueError("quantity must be positive")
gd1 = |     if item.quantity > 10_000:
gd2 = |         raise ValueError("invalid item")
ge1 = |     if item.price_cents == 0:
ge2 = |         log_rejection(item)
ge3 = |         raise ValueError("invalid item")
gf1 = |     if item.price_cents > 1_000_000:
gf2 = |         raise ValueError("invalid item")
""",
            },
        ),
    ],
)

TYPESCRIPT = Fixture(
    mutation_class="duplicate-heavy",
    language="typescript",
    summary=(
        "validate has six guard blocks with an identical `throw new Error(\"invalid item\");` line and an "
        "identical closing `  }`; two of them also log with an identical `logRejection(item);`. Only the "
        "third block is edited: a born `logRejection(item);` (identical to the two existing ones) is "
        "inserted, and its throw message is edited in place. Every later line shifts down by one. A "
        "matcher that pairs identical lines as a set, or by line number, mislabels the duplicates."
    ),
    commits=[
        Commit(
            message="add item validation",
            files={
                "src/validate.ts": r"""
u1  + | import type { Item } from "./inventory";
      |
l1  + | function logRejection(item: Item): void {
l2  + |   console.error(`rejected ${item.name}`);
l3  + | }
      |
fn1 + | export function validate(item: Item): void {
ga1 + |   if (item.name === "") {
ga2 + |     throw new Error("invalid item");
ga3 + |   }
gb1 + |   if (item.name.length > 64) {
gb2 + |     logRejection(item);
gb3 + |     throw new Error("invalid item");
gb4 + |   }
gc1 + |   if (item.quantity === 0) {
gc2 + |     throw new Error("invalid item");
gc3 + |   }
gd1 + |   if (item.quantity > 10_000) {
gd2 + |     throw new Error("invalid item");
gd3 + |   }
ge1 + |   if (item.priceCents === 0) {
ge2 + |     logRejection(item);
ge3 + |     throw new Error("invalid item");
ge4 + |   }
gf1 + |   if (item.priceCents > 1_000_000) {
gf2 + |     throw new Error("invalid item");
gf3 + |   }
fn2 + | }
""",
            },
        ),
        Commit(
            message="explain zero-quantity rejections",
            files={
                "src/validate.ts": r"""
u1  = | import type { Item } from "./inventory";
      |
l1  = | function logRejection(item: Item): void {
l2  = |   console.error(`rejected ${item.name}`);
l3  = | }
      |
fn1 = | export function validate(item: Item): void {
ga1 = |   if (item.name === "") {
ga2 = |     throw new Error("invalid item");
ga3 = |   }
gb1 = |   if (item.name.length > 64) {
gb2 = |     logRejection(item);
gb3 = |     throw new Error("invalid item");
gb4 = |   }
gc1 = |   if (item.quantity === 0) {
n1  + |     logRejection(item);
gc2 ~ |     throw new Error("quantity must be positive");
gc3 = |   }
gd1 = |   if (item.quantity > 10_000) {
gd2 = |     throw new Error("invalid item");
gd3 = |   }
ge1 = |   if (item.priceCents === 0) {
ge2 = |     logRejection(item);
ge3 = |     throw new Error("invalid item");
ge4 = |   }
gf1 = |   if (item.priceCents > 1_000_000) {
gf2 = |     throw new Error("invalid item");
gf3 = |   }
fn2 = | }
""",
            },
        ),
    ],
)

FIXTURES = [RUST, PYTHON, TYPESCRIPT]
