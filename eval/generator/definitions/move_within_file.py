from fixture_lib import Commit, Fixture

RUST = Fixture(
    mutation_class="move-within-file",
    language="rust",
    summary=(
        "Move the 5-line helper normalize_name from the top of the file to the bottom, past 17 stable "
        "lines. Content unchanged. The helper's lines are moved; every other line is verbatim."
    ),
    commits=[
        Commit(
            message="add inventory parser",
            files={
                "src/inventory.rs": r"""
u1 + | use std::collections::HashMap;
     |
n1 + | fn normalize_name(raw: &str) -> String {
n2 + |     let trimmed = raw.trim();
n3 + |     let lowered = trimmed.to_lowercase();
n4 + |     lowered.replace(' ', "_")
n5 + | }
     |
s1 + | pub struct Item {
s2 + |     pub name: String,
s3 + |     pub quantity: u32,
s4 + | }
     |
p1 + | pub fn parse_item(line: &str) -> Option<Item> {
p2 + |     let mut parts = line.split(',');
p3 + |     let name = normalize_name(parts.next()?);
p4 + |     let quantity = parts.next()?.trim().parse().ok()?;
p5 + |     Some(Item { name, quantity })
p6 + | }
     |
i1 + | pub fn index_by_name(items: Vec<Item>) -> HashMap<String, Item> {
i2 + |     let mut index = HashMap::new();
i3 + |     for item in items {
i4 + |         index.insert(item.name.clone(), item);
i5 + |     }
i6 + |     index
i7 + | }
""",
            },
        ),
        Commit(
            message="move normalize_name below its callers",
            files={
                "src/inventory.rs": r"""
u1 = | use std::collections::HashMap;
     |
s1 = | pub struct Item {
s2 = |     pub name: String,
s3 = |     pub quantity: u32,
s4 = | }
     |
p1 = | pub fn parse_item(line: &str) -> Option<Item> {
p2 = |     let mut parts = line.split(',');
p3 = |     let name = normalize_name(parts.next()?);
p4 = |     let quantity = parts.next()?.trim().parse().ok()?;
p5 = |     Some(Item { name, quantity })
p6 = | }
     |
i1 = | pub fn index_by_name(items: Vec<Item>) -> HashMap<String, Item> {
i2 = |     let mut index = HashMap::new();
i3 = |     for item in items {
i4 = |         index.insert(item.name.clone(), item);
i5 = |     }
i6 = |     index
i7 = | }
     |
n1 > | fn normalize_name(raw: &str) -> String {
n2 > |     let trimmed = raw.trim();
n3 > |     let lowered = trimmed.to_lowercase();
n4 > |     lowered.replace(' ', "_")
n5 > | }
""",
            },
        ),
    ],
)

PYTHON = Fixture(
    mutation_class="move-within-file",
    language="python",
    summary=(
        "Move the 4-line helper normalize_name from the top of the file to between parse_item and "
        "index_by_name, past 9 stable lines with 5 more stable lines after it. Content unchanged. The "
        "helper's lines are moved; every other line is verbatim."
    ),
    commits=[
        Commit(
            message="add inventory parser",
            files={
                "inventory.py": r"""
f1 + | from dataclasses import dataclass
f2 + | from typing import Dict, List, Optional
     |
     |
n1 + | def normalize_name(raw: str) -> str:
n2 + |     trimmed = raw.strip()
n3 + |     lowered = trimmed.lower()
n4 + |     return lowered.replace(" ", "_")
     |
     |
k1 + | @dataclass
k2 + | class Item:
k3 + |     name: str
k4 + |     quantity: int
     |
     |
p1 + | def parse_item(line: str) -> Optional[Item]:
p2 + |     parts = line.split(",")
p3 + |     if len(parts) < 2:
p4 + |         return None
p5 + |     return Item(normalize_name(parts[0]), int(parts[1]))
     |
     |
i1 + | def index_by_name(items: List[Item]) -> Dict[str, Item]:
i2 + |     index: Dict[str, Item] = {}
i3 + |     for item in items:
i4 + |         index[item.name] = item
i5 + |     return index
""",
            },
        ),
        Commit(
            message="move normalize_name next to parse_item",
            files={
                "inventory.py": r"""
f1 = | from dataclasses import dataclass
f2 = | from typing import Dict, List, Optional
     |
     |
k1 = | @dataclass
k2 = | class Item:
k3 = |     name: str
k4 = |     quantity: int
     |
     |
p1 = | def parse_item(line: str) -> Optional[Item]:
p2 = |     parts = line.split(",")
p3 = |     if len(parts) < 2:
p4 = |         return None
p5 = |     return Item(normalize_name(parts[0]), int(parts[1]))
     |
     |
n1 > | def normalize_name(raw: str) -> str:
n2 > |     trimmed = raw.strip()
n3 > |     lowered = trimmed.lower()
n4 > |     return lowered.replace(" ", "_")
     |
     |
i1 = | def index_by_name(items: List[Item]) -> Dict[str, Item]:
i2 = |     index: Dict[str, Item] = {}
i3 = |     for item in items:
i4 = |         index[item.name] = item
i5 = |     return index
""",
            },
        ),
    ],
)

TYPESCRIPT = Fixture(
    mutation_class="move-within-file",
    language="typescript",
    summary=(
        "Move the 5-line helper normalizeName from the bottom of the file to the top, past 18 stable "
        "lines. Content unchanged. The helper's lines are moved; every other line is verbatim."
    ),
    commits=[
        Commit(
            message="add inventory parser",
            files={
                "src/inventory.ts": r"""
s1 + | export interface Item {
s2 + |   name: string;
s3 + |   quantity: number;
s4 + | }
     |
p1 + | export function parseItem(line: string): Item | undefined {
p2 + |   const parts = line.split(",");
p3 + |   if (parts.length < 2) {
p4 + |     return undefined;
p5 + |   }
p6 + |   return { name: normalizeName(parts[0]), quantity: Number(parts[1]) };
p7 + | }
     |
i1 + | export function indexByName(items: Item[]): Map<string, Item> {
i2 + |   const index = new Map<string, Item>();
i3 + |   for (const item of items) {
i4 + |     index.set(item.name, item);
i5 + |   }
i6 + |   return index;
i7 + | }
     |
n1 + | function normalizeName(raw: string): string {
n2 + |   const trimmed = raw.trim();
n3 + |   const lowered = trimmed.toLowerCase();
n4 + |   return lowered.replace(/ /g, "_");
n5 + | }
""",
            },
        ),
        Commit(
            message="move normalizeName above its callers",
            files={
                "src/inventory.ts": r"""
n1 > | function normalizeName(raw: string): string {
n2 > |   const trimmed = raw.trim();
n3 > |   const lowered = trimmed.toLowerCase();
n4 > |   return lowered.replace(/ /g, "_");
n5 > | }
     |
s1 = | export interface Item {
s2 = |   name: string;
s3 = |   quantity: number;
s4 = | }
     |
p1 = | export function parseItem(line: string): Item | undefined {
p2 = |   const parts = line.split(",");
p3 = |   if (parts.length < 2) {
p4 = |     return undefined;
p5 = |   }
p6 = |   return { name: normalizeName(parts[0]), quantity: Number(parts[1]) };
p7 = | }
     |
i1 = | export function indexByName(items: Item[]): Map<string, Item> {
i2 = |   const index = new Map<string, Item>();
i3 = |   for (const item of items) {
i4 = |     index.set(item.name, item);
i5 = |   }
i6 = |   return index;
i7 = | }
""",
            },
        ),
    ],
)

FIXTURES = [RUST, PYTHON, TYPESCRIPT]
