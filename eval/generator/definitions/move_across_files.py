from fixture_lib import Commit, Fixture

RUST = Fixture(
    mutation_class="move-across-files",
    language="rust",
    summary=(
        "Extract fn total_value from src/inventory.rs into a new src/pricing.rs. Its 6 lines are moved "
        "unchanged; the new file's import and the new mod declaration are born; the import of total_value "
        "in main.rs is edited to the new module. most_valuable stays behind and shares the identical lines "
        "`items` and `.iter()` with the moved function."
    ),
    commits=[
        Commit(
            message="add inventory valuation",
            files={
                "src/inventory.rs": r"""
s1 + | pub struct Item {
s2 + |     pub name: String,
s3 + |     pub quantity: u32,
s4 + |     pub price_cents: u64,
s5 + | }
     |
p1 + | pub fn parse_item(line: &str) -> Option<Item> {
p2 + |     let mut parts = line.split(',');
p3 + |     let name = parts.next()?.trim().to_string();
p4 + |     let quantity = parts.next()?.trim().parse().ok()?;
p5 + |     let price_cents = parts.next()?.trim().parse().ok()?;
p6 + |     Some(Item { name, quantity, price_cents })
p7 + | }
     |
t1 + | pub fn total_value(items: &[Item]) -> u64 {
t2 + |     items
t3 + |         .iter()
t4 + |         .map(|item| item.quantity as u64 * item.price_cents)
t5 + |         .sum()
t6 + | }
     |
v1 + | pub fn most_valuable(items: &[Item]) -> Option<&Item> {
v2 + |     items
v3 + |         .iter()
v4 + |         .max_by_key(|item| item.quantity as u64 * item.price_cents)
v5 + | }
""",
                "src/main.rs": r"""
m1 + | mod inventory;
     |
m2 + | use inventory::parse_item;
m3 + | use inventory::total_value;
     |
m4 + | fn main() {
m5 + |     let input = std::fs::read_to_string("items.csv").expect("read items.csv");
m6 + |     let items: Vec<_> = input.lines().filter_map(parse_item).collect();
m7 + |     println!("total: {}", total_value(&items));
m8 + | }
""",
            },
        ),
        Commit(
            message="extract pricing module",
            files={
                "src/inventory.rs": r"""
s1 = | pub struct Item {
s2 = |     pub name: String,
s3 = |     pub quantity: u32,
s4 = |     pub price_cents: u64,
s5 = | }
     |
p1 = | pub fn parse_item(line: &str) -> Option<Item> {
p2 = |     let mut parts = line.split(',');
p3 = |     let name = parts.next()?.trim().to_string();
p4 = |     let quantity = parts.next()?.trim().parse().ok()?;
p5 = |     let price_cents = parts.next()?.trim().parse().ok()?;
p6 = |     Some(Item { name, quantity, price_cents })
p7 = | }
     |
v1 = | pub fn most_valuable(items: &[Item]) -> Option<&Item> {
v2 = |     items
v3 = |         .iter()
v4 = |         .max_by_key(|item| item.quantity as u64 * item.price_cents)
v5 = | }
""",
                "src/pricing.rs": r"""
r1 + | use crate::inventory::Item;
     |
t1 > | pub fn total_value(items: &[Item]) -> u64 {
t2 > |     items
t3 > |         .iter()
t4 > |         .map(|item| item.quantity as u64 * item.price_cents)
t5 > |         .sum()
t6 > | }
""",
                "src/main.rs": r"""
m1 = | mod inventory;
r2 + | mod pricing;
     |
m2 = | use inventory::parse_item;
m3 ~ | use pricing::total_value;
     |
m4 = | fn main() {
m5 = |     let input = std::fs::read_to_string("items.csv").expect("read items.csv");
m6 = |     let items: Vec<_> = input.lines().filter_map(parse_item).collect();
m7 = |     println!("total: {}", total_value(&items));
m8 = | }
""",
            },
        ),
    ],
)

PYTHON = Fixture(
    mutation_class="move-across-files",
    language="python",
    summary=(
        "Extract function total_value from inventory.py into a new pricing.py. Its 5 lines are moved "
        "unchanged; the new file's imports are born; the import of total_value in main.py is edited to "
        "the new module. index_by_name stays behind and shares the identical line "
        "`for item in items:` with the moved function."
    ),
    commits=[
        Commit(
            message="add inventory valuation",
            files={
                "inventory.py": r"""
f1 + | from dataclasses import dataclass
f2 + | from typing import Dict, Iterable, List, Optional
     |
     |
k1 + | @dataclass
k2 + | class Item:
k3 + |     name: str
k4 + |     quantity: int
k5 + |     price_cents: int
     |
     |
p1 + | def parse_rows(rows: Iterable[str]) -> List[Item]:
p2 + |     items = []
p3 + |     for row in rows:
p4 + |         name, quantity, price_cents = row.split(",")
p5 + |         items.append(Item(name.strip(), int(quantity), int(price_cents)))
p6 + |     return items
     |
     |
t1 + | def total_value(items: List[Item]) -> int:
t2 + |     total = 0
t3 + |     for item in items:
t4 + |         total += item.quantity * item.price_cents
t5 + |     return total
     |
     |
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
m1 + | import sys
     |
m2 + | from inventory import parse_rows
m3 + | from inventory import total_value
     |
     |
m4 + | def main() -> None:
m5 + |     with open(sys.argv[1]) as handle:
m6 + |         items = parse_rows(handle)
m7 + |     print(f"total: {total_value(items)}")
     |
     |
m8 + | if __name__ == "__main__":
m9 + |     main()
""",
            },
        ),
        Commit(
            message="extract pricing module",
            files={
                "inventory.py": r"""
f1 = | from dataclasses import dataclass
f2 = | from typing import Dict, Iterable, List, Optional
     |
     |
k1 = | @dataclass
k2 = | class Item:
k3 = |     name: str
k4 = |     quantity: int
k5 = |     price_cents: int
     |
     |
p1 = | def parse_rows(rows: Iterable[str]) -> List[Item]:
p2 = |     items = []
p3 = |     for row in rows:
p4 = |         name, quantity, price_cents = row.split(",")
p5 = |         items.append(Item(name.strip(), int(quantity), int(price_cents)))
p6 = |     return items
     |
     |
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
                "pricing.py": r"""
r1 + | from typing import List
     |
r2 + | from inventory import Item
     |
     |
t1 > | def total_value(items: List[Item]) -> int:
t2 > |     total = 0
t3 > |     for item in items:
t4 > |         total += item.quantity * item.price_cents
t5 > |     return total
""",
                "main.py": r"""
m1 = | import sys
     |
m2 = | from inventory import parse_rows
m3 ~ | from pricing import total_value
     |
     |
m4 = | def main() -> None:
m5 = |     with open(sys.argv[1]) as handle:
m6 = |         items = parse_rows(handle)
m7 = |     print(f"total: {total_value(items)}")
     |
     |
m8 = | if __name__ == "__main__":
m9 = |     main()
""",
            },
        ),
    ],
)

TYPESCRIPT = Fixture(
    mutation_class="move-across-files",
    language="typescript",
    summary=(
        "Extract function totalValue from src/inventory.ts into a new src/pricing.ts. Its 6 lines are "
        "moved unchanged; the new file's import is born; the import of totalValue in main.ts is edited to "
        "the new module. totalQuantity stays behind and shares the identical lines "
        "`return items.reduce(`, `0,`, `);`, and `}` with the moved function."
    ),
    commits=[
        Commit(
            message="add inventory valuation",
            files={
                "src/inventory.ts": r"""
s1 + | export interface Item {
s2 + |   name: string;
s3 + |   quantity: number;
s4 + |   priceCents: number;
s5 + | }
     |
p1 + | export function parseItem(line: string): Item | undefined {
p2 + |   const [name, quantity, priceCents] = line.split(",");
p3 + |   if (priceCents === undefined) {
p4 + |     return undefined;
p5 + |   }
p6 + |   return { name: name.trim(), quantity: +quantity, priceCents: +priceCents };
p7 + | }
     |
t1 + | export function totalValue(items: Item[]): number {
t2 + |   return items.reduce(
t3 + |     (sum, item) => sum + item.quantity * item.priceCents,
t4 + |     0,
t5 + |   );
t6 + | }
     |
q1 + | export function totalQuantity(items: Item[]): number {
q2 + |   return items.reduce(
q3 + |     (sum, item) => sum + item.quantity,
q4 + |     0,
q5 + |   );
q6 + | }
""",
                "src/main.ts": r"""
m1 + | import { readFileSync } from "fs";
m2 + | import { parseItem } from "./inventory";
m3 + | import { totalValue } from "./inventory";
     |
m4 + | const lines = readFileSync(process.argv[2], "utf8").split("\n");
m5 + | const items = lines.flatMap((line) => parseItem(line) ?? []);
m6 + | console.log(`total: ${totalValue(items)}`);
""",
            },
        ),
        Commit(
            message="extract pricing module",
            files={
                "src/inventory.ts": r"""
s1 = | export interface Item {
s2 = |   name: string;
s3 = |   quantity: number;
s4 = |   priceCents: number;
s5 = | }
     |
p1 = | export function parseItem(line: string): Item | undefined {
p2 = |   const [name, quantity, priceCents] = line.split(",");
p3 = |   if (priceCents === undefined) {
p4 = |     return undefined;
p5 = |   }
p6 = |   return { name: name.trim(), quantity: +quantity, priceCents: +priceCents };
p7 = | }
     |
q1 = | export function totalQuantity(items: Item[]): number {
q2 = |   return items.reduce(
q3 = |     (sum, item) => sum + item.quantity,
q4 = |     0,
q5 = |   );
q6 = | }
""",
                "src/pricing.ts": r"""
r1 + | import type { Item } from "./inventory";
     |
t1 > | export function totalValue(items: Item[]): number {
t2 > |   return items.reduce(
t3 > |     (sum, item) => sum + item.quantity * item.priceCents,
t4 > |     0,
t5 > |   );
t6 > | }
""",
                "src/main.ts": r"""
m1 = | import { readFileSync } from "fs";
m2 = | import { parseItem } from "./inventory";
m3 ~ | import { totalValue } from "./pricing";
     |
m4 = | const lines = readFileSync(process.argv[2], "utf8").split("\n");
m5 = | const items = lines.flatMap((line) => parseItem(line) ?? []);
m6 = | console.log(`total: ${totalValue(items)}`);
""",
            },
        ),
    ],
)

FIXTURES = [RUST, PYTHON, TYPESCRIPT]
