from fixture_lib import Commit, Fixture

RUST = Fixture(
    mutation_class="reindent",
    language="rust",
    summary=(
        "Wrap a for loop and the statement after it in `if !items.is_empty() { ... }`. The 4 wrapped lines "
        "are verbatim one indent level deeper; the `if` line and its closing brace are born. A verbatim "
        "statement separates the loop's `}` from the new `}`, so which brace is new is unambiguous."
    ),
    commits=[
        Commit(
            message="add inventory index",
            files={
                "src/inventory.rs": r"""
u1 + | use std::collections::HashMap;
     |
s1 + | pub struct Item {
s2 + |     pub name: String,
s3 + |     pub quantity: u32,
s4 + | }
     |
i1 + | pub fn index_by_name(items: Vec<Item>) -> HashMap<String, Item> {
i2 + |     let mut index = HashMap::new();
i3 + |     for item in items {
i4 + |         index.insert(item.name.clone(), item);
i5 + |     }
i6 + |     println!("indexed {} items", index.len());
i7 + |     index
i8 + | }
     |
r1 + | pub fn restock(items: &mut [Item], amount: u32) {
r2 + |     for item in items.iter_mut() {
r3 + |         item.quantity += amount;
r4 + |     }
r5 + | }
""",
            },
        ),
        Commit(
            message="skip indexing log for empty input",
            files={
                "src/inventory.rs": r"""
u1 = | use std::collections::HashMap;
     |
s1 = | pub struct Item {
s2 = |     pub name: String,
s3 = |     pub quantity: u32,
s4 = | }
     |
i1 = | pub fn index_by_name(items: Vec<Item>) -> HashMap<String, Item> {
i2 = |     let mut index = HashMap::new();
w1 + |     if !items.is_empty() {
i3 = |         for item in items {
i4 = |             index.insert(item.name.clone(), item);
i5 = |         }
i6 = |         println!("indexed {} items", index.len());
w2 + |     }
i7 = |     index
i8 = | }
     |
r1 = | pub fn restock(items: &mut [Item], amount: u32) {
r2 = |     for item in items.iter_mut() {
r3 = |         item.quantity += amount;
r4 = |     }
r5 = | }
""",
            },
        ),
    ],
)

PYTHON = Fixture(
    mutation_class="reindent",
    language="python",
    summary=(
        "Wrap two blocks in new conditionals (`if items:` around a loop and the print after it, "
        "`if amount > 0:` around a loop). The 5 wrapped lines are verbatim one indent level deeper; the "
        "two `if` lines are born. Both wrapped loops start with the identical line `for item in items:`."
    ),
    commits=[
        Commit(
            message="add inventory index",
            files={
                "inventory.py": r"""
f1 + | from dataclasses import dataclass
f2 + | from typing import Dict, List
     |
     |
k1 + | @dataclass
k2 + | class Item:
k3 + |     name: str
k4 + |     quantity: int
     |
     |
i1 + | def index_by_name(items: List[Item]) -> Dict[str, Item]:
i2 + |     index: Dict[str, Item] = {}
i3 + |     for item in items:
i4 + |         index[item.name] = item
i5 + |     print(f"indexed {len(index)} items")
i6 + |     return index
     |
     |
r1 + | def restock(items: List[Item], amount: int) -> None:
r2 + |     for item in items:
r3 + |         item.quantity += amount
""",
            },
        ),
        Commit(
            message="guard indexing and restocking",
            files={
                "inventory.py": r"""
f1 = | from dataclasses import dataclass
f2 = | from typing import Dict, List
     |
     |
k1 = | @dataclass
k2 = | class Item:
k3 = |     name: str
k4 = |     quantity: int
     |
     |
i1 = | def index_by_name(items: List[Item]) -> Dict[str, Item]:
i2 = |     index: Dict[str, Item] = {}
w1 + |     if items:
i3 = |         for item in items:
i4 = |             index[item.name] = item
i5 = |         print(f"indexed {len(index)} items")
i6 = |     return index
     |
     |
r1 = | def restock(items: List[Item], amount: int) -> None:
w2 + |     if amount > 0:
r2 = |         for item in items:
r3 = |             item.quantity += amount
""",
            },
        ),
    ],
)

TYPESCRIPT = Fixture(
    mutation_class="reindent",
    language="typescript",
    summary=(
        "Wrap a for loop and the statement after it in `if (items.length > 0) { ... }`. The 4 wrapped "
        "lines are verbatim one indent level deeper; the `if` line and its closing brace are born. A "
        "verbatim statement separates the loop's `}` from the new `}`, so which brace is new is unambiguous."
    ),
    commits=[
        Commit(
            message="add inventory index",
            files={
                "src/inventory.ts": r"""
s1 + | export interface Item {
s2 + |   name: string;
s3 + |   quantity: number;
s4 + | }
     |
i1 + | export function indexByName(items: Item[]): Map<string, Item> {
i2 + |   const index = new Map<string, Item>();
i3 + |   for (const item of items) {
i4 + |     index.set(item.name, item);
i5 + |   }
i6 + |   console.log(`indexed ${index.size} items`);
i7 + |   return index;
i8 + | }
     |
r1 + | export function restock(items: Item[], amount: number): void {
r2 + |   for (const item of items) {
r3 + |     item.quantity += amount;
r4 + |   }
r5 + | }
""",
            },
        ),
        Commit(
            message="skip indexing log for empty input",
            files={
                "src/inventory.ts": r"""
s1 = | export interface Item {
s2 = |   name: string;
s3 = |   quantity: number;
s4 = | }
     |
i1 = | export function indexByName(items: Item[]): Map<string, Item> {
i2 = |   const index = new Map<string, Item>();
w1 + |   if (items.length > 0) {
i3 = |     for (const item of items) {
i4 = |       index.set(item.name, item);
i5 = |     }
i6 = |     console.log(`indexed ${index.size} items`);
w2 + |   }
i7 = |   return index;
i8 = | }
     |
r1 = | export function restock(items: Item[], amount: number): void {
r2 = |   for (const item of items) {
r3 = |     item.quantity += amount;
r4 = |   }
r5 = | }
""",
            },
        ),
    ],
)

FIXTURES = [RUST, PYTHON, TYPESCRIPT]
