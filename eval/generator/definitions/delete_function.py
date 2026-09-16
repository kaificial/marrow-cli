from fixture_lib import UNCHANGED, Commit, Fixture

RUST = Fixture(
    mutation_class="delete-function",
    language="rust",
    summary=(
        "Delete fn total_value and its doc comment (7 lines die). Nothing else changes. The surviving "
        "most_valuable has identical `items` and `.iter()` lines that shift up into the deleted function's "
        "old line numbers; main.rs is untouched."
    ),
    commits=[
        Commit(
            message="add inventory valuation",
            files={
                "src/inventory.rs": r"""
u1 + | use std::collections::HashMap;
     |
s1 + | pub struct Item {
s2 + |     pub name: String,
s3 + |     pub quantity: u32,
s4 + |     pub price_cents: u64,
s5 + | }
     |
t0 + | /// Sum of quantity times unit price across all items.
t1 + | pub fn total_value(items: &[Item]) -> u64 {
t2 + |     items
t3 + |         .iter()
t4 + |         .map(|item| item.quantity as u64 * item.price_cents)
t5 + |         .sum()
t6 + | }
     |
v0 + | /// The item with the highest stock value, if any.
v1 + | pub fn most_valuable(items: &[Item]) -> Option<&Item> {
v2 + |     items
v3 + |         .iter()
v4 + |         .max_by_key(|item| item.quantity as u64 * item.price_cents)
v5 + | }
     |
i1 + | pub fn index_by_name(items: Vec<Item>) -> HashMap<String, Item> {
i2 + |     let mut index = HashMap::new();
i3 + |     for item in items {
i4 + |         index.insert(item.name.clone(), item);
i5 + |     }
i6 + |     index
i7 + | }
""",
                "src/main.rs": r"""
m1  + | mod inventory;
      |
m2  + | fn main() {
m3  + |     let items = vec![inventory::Item {
m4  + |         name: String::from("widget"),
m5  + |         quantity: 3,
m6  + |         price_cents: 250,
m7  + |     }];
m8  + |     if let Some(item) = inventory::most_valuable(&items) {
m9  + |         println!("most valuable: {}", item.name);
m10 + |     }
m11 + | }
""",
            },
        ),
        Commit(
            message="remove unused total_value",
            dead="t0 t1 t2 t3 t4 t5 t6",
            files={
                "src/inventory.rs": r"""
u1 = | use std::collections::HashMap;
     |
s1 = | pub struct Item {
s2 = |     pub name: String,
s3 = |     pub quantity: u32,
s4 = |     pub price_cents: u64,
s5 = | }
     |
v0 = | /// The item with the highest stock value, if any.
v1 = | pub fn most_valuable(items: &[Item]) -> Option<&Item> {
v2 = |     items
v3 = |         .iter()
v4 = |         .max_by_key(|item| item.quantity as u64 * item.price_cents)
v5 = | }
     |
i1 = | pub fn index_by_name(items: Vec<Item>) -> HashMap<String, Item> {
i2 = |     let mut index = HashMap::new();
i3 = |     for item in items {
i4 = |         index.insert(item.name.clone(), item);
i5 = |     }
i6 = |     index
i7 = | }
""",
                "src/main.rs": UNCHANGED,
            },
        ),
    ],
)

PYTHON = Fixture(
    mutation_class="delete-function",
    language="python",
    summary=(
        "Delete function total_value and its comment (5 lines die). Nothing else changes. The surviving "
        "most_valuable has identical `if not items:` guard lines that shift up into the deleted function's "
        "old line numbers; main.py is untouched."
    ),
    commits=[
        Commit(
            message="add inventory valuation",
            files={
                "inventory.py": r"""
f1 + | from dataclasses import dataclass
f2 + | from typing import Dict, List, Optional
     |
     |
k1 + | @dataclass
k2 + | class Item:
k3 + |     name: str
k4 + |     quantity: int
k5 + |     price_cents: int
     |
     |
t0 + | # Sum of quantity times unit price across all items.
t1 + | def total_value(items: List[Item]) -> int:
t2 + |     if not items:
t3 + |         return 0
t4 + |     return sum(item.quantity * item.price_cents for item in items)
     |
     |
v0 + | # The item with the highest stock value, if any.
v1 + | def most_valuable(items: List[Item]) -> Optional[Item]:
v2 + |     if not items:
v3 + |         return None
v4 + |     return max(items, key=lambda item: item.quantity * item.price_cents)
     |
     |
i1 + | def index_by_name(items: List[Item]) -> Dict[str, Item]:
i2 + |     index: Dict[str, Item] = {}
i3 + |     for item in items:
i4 + |         index[item.name] = item
i5 + |     return index
""",
                "main.py": r"""
m1 + | from inventory import Item, most_valuable
     |
     |
m2 + | def main() -> None:
m3 + |     items = [Item("widget", 3, 250), Item("gadget", 1, 900)]
m4 + |     best = most_valuable(items)
m5 + |     print(best.name if best else "empty")
     |
     |
m6 + | if __name__ == "__main__":
m7 + |     main()
""",
            },
        ),
        Commit(
            message="remove unused total_value",
            dead="t0 t1 t2 t3 t4",
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
k5 = |     price_cents: int
     |
     |
v0 = | # The item with the highest stock value, if any.
v1 = | def most_valuable(items: List[Item]) -> Optional[Item]:
v2 = |     if not items:
v3 = |         return None
v4 = |     return max(items, key=lambda item: item.quantity * item.price_cents)
     |
     |
i1 = | def index_by_name(items: List[Item]) -> Dict[str, Item]:
i2 = |     index: Dict[str, Item] = {}
i3 = |     for item in items:
i4 = |         index[item.name] = item
i5 = |     return index
""",
                "main.py": UNCHANGED,
            },
        ),
    ],
)

TYPESCRIPT = Fixture(
    mutation_class="delete-function",
    language="typescript",
    summary=(
        "Delete function totalValue and its comment (8 lines die). Nothing else changes. The surviving "
        "totalQuantity is identical except for its comment, signature, and accumulator line, so 5 of its "
        "lines duplicate deleted ones and shift up into their old line numbers; main.ts is untouched."
    ),
    commits=[
        Commit(
            message="add inventory totals",
            files={
                "src/inventory.ts": r"""
s1 + | export interface Item {
s2 + |   name: string;
s3 + |   quantity: number;
s4 + |   priceCents: number;
s5 + | }
     |
t0 + | // Sum of quantity times unit price across all items.
t1 + | export function totalValue(items: Item[]): number {
t2 + |   let total = 0;
t3 + |   for (const item of items) {
t4 + |     total += item.quantity * item.priceCents;
t5 + |   }
t6 + |   return total;
t7 + | }
     |
q0 + | // Total units in stock across all items.
q1 + | export function totalQuantity(items: Item[]): number {
q2 + |   let total = 0;
q3 + |   for (const item of items) {
q4 + |     total += item.quantity;
q5 + |   }
q6 + |   return total;
q7 + | }
     |
i1 + | export function indexByName(items: Item[]): Map<string, Item> {
i2 + |   const index = new Map<string, Item>();
i3 + |   for (const item of items) {
i4 + |     index.set(item.name, item);
i5 + |   }
i6 + |   return index;
i7 + | }
""",
                "src/main.ts": r"""
m1 + | import { indexByName, totalQuantity } from "./inventory";
     |
m2 + | const items = [{ name: "widget", quantity: 3, priceCents: 250 }];
m3 + | console.log(totalQuantity(items), indexByName(items).size);
""",
            },
        ),
        Commit(
            message="remove unused totalValue",
            dead="t0 t1 t2 t3 t4 t5 t6 t7",
            files={
                "src/inventory.ts": r"""
s1 = | export interface Item {
s2 = |   name: string;
s3 = |   quantity: number;
s4 = |   priceCents: number;
s5 = | }
     |
q0 = | // Total units in stock across all items.
q1 = | export function totalQuantity(items: Item[]): number {
q2 = |   let total = 0;
q3 = |   for (const item of items) {
q4 = |     total += item.quantity;
q5 = |   }
q6 = |   return total;
q7 = | }
     |
i1 = | export function indexByName(items: Item[]): Map<string, Item> {
i2 = |   const index = new Map<string, Item>();
i3 = |   for (const item of items) {
i4 = |     index.set(item.name, item);
i5 = |   }
i6 = |   return index;
i7 = | }
""",
                "src/main.ts": UNCHANGED,
            },
        ),
    ],
)

FIXTURES = [RUST, PYTHON, TYPESCRIPT]
