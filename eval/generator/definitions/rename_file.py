from fixture_lib import Commit, Fixture

RUST = Fixture(
    mutation_class="rename-file",
    language="rust",
    summary=(
        "Rename src/inventory.rs to src/stock.rs: its lines are verbatim at the new path except the module doc "
        "comment, which is edited; `mod` and `use` in main.rs are edited. In the same commit an unrelated "
        "src/bin/legacy_import.rs is deleted (all lines die) and src/bin/report.rs is added (all lines born); "
        "that pair shares only the single lines `fn main() {` and `}` and must not be treated as a rename."
    ),
    commits=[
        Commit(
            message="add inventory parser and legacy import tool",
            files={
                "src/inventory.rs": r"""
d1 + | //! Inventory rows parsed from CSV.
     |
s1 + | pub struct Item {
s2 + |     pub name: String,
s3 + |     pub quantity: u32,
s4 + | }
     |
p1 + | pub fn parse_item(line: &str) -> Option<Item> {
p2 + |     let mut parts = line.split(',');
p3 + |     let name = parts.next()?.trim().to_string();
p4 + |     let quantity = parts.next()?.trim().parse().ok()?;
p5 + |     Some(Item { name, quantity })
p6 + | }
""",
                "src/main.rs": r"""
m1 + | mod inventory;
     |
m2 + | use inventory::parse_item;
     |
m3 + | fn main() {
m4 + |     let line = std::env::args().nth(1).unwrap_or_default();
m5 + |     if let Some(item) = parse_item(&line) {
m6 + |         println!("{} x{}", item.name, item.quantity);
m7 + |     }
m8 + | }
""",
                "src/bin/legacy_import.rs": r"""
g1 + | use std::io::Read;
     |
g2 + | fn main() {
g3 + |     let mut raw = String::new();
g4 + |     std::io::stdin().read_to_string(&mut raw).unwrap();
g5 + |     let rows = raw.split(';').filter(|row| !row.is_empty()).count();
g6 + |     println!("{rows} legacy rows");
g7 + | }
""",
            },
        ),
        Commit(
            message="rename inventory module to stock; replace legacy import with report",
            renames={"src/inventory.rs": "src/stock.rs"},
            dead="g1 g2 g3 g4 g5 g6 g7",
            files={
                "src/stock.rs": r"""
d1 ~ | //! Stock rows parsed from CSV.
     |
s1 = | pub struct Item {
s2 = |     pub name: String,
s3 = |     pub quantity: u32,
s4 = | }
     |
p1 = | pub fn parse_item(line: &str) -> Option<Item> {
p2 = |     let mut parts = line.split(',');
p3 = |     let name = parts.next()?.trim().to_string();
p4 = |     let quantity = parts.next()?.trim().parse().ok()?;
p5 = |     Some(Item { name, quantity })
p6 = | }
""",
                "src/main.rs": r"""
m1 ~ | mod stock;
     |
m2 ~ | use stock::parse_item;
     |
m3 = | fn main() {
m4 = |     let line = std::env::args().nth(1).unwrap_or_default();
m5 = |     if let Some(item) = parse_item(&line) {
m6 = |         println!("{} x{}", item.name, item.quantity);
m7 = |     }
m8 = | }
""",
                "src/bin/report.rs": r"""
q1  + | use std::collections::BTreeMap;
      |
q2  + | fn main() {
q3  + |     let mut totals: BTreeMap<String, u32> = BTreeMap::new();
q4  + |     for arg in std::env::args().skip(1) {
q5  + |         *totals.entry(arg).or_default() += 1;
q6  + |     }
q7  + |     for (name, count) in &totals {
q8  + |         println!("{name}: {count}");
q9  + |     }
q10 + | }
""",
            },
        ),
    ],
)

PYTHON = Fixture(
    mutation_class="rename-file",
    language="python",
    summary=(
        "Rename inventory.py to stock.py: its lines are verbatim at the new path except the header comment, "
        "which is edited; the import in main.py is edited. In the same commit an unrelated "
        "scripts/legacy_import.py is deleted (all lines die) and scripts/report.py is added (all lines born); "
        "that pair shares only the single line `import sys` and must not be treated as a rename."
    ),
    commits=[
        Commit(
            message="add inventory parser and legacy import script",
            files={
                "inventory.py": r"""
d1 + | # Inventory rows parsed from CSV.
f1 + | from dataclasses import dataclass
f2 + | from typing import Optional
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
p5 + |     return Item(parts[0].strip(), int(parts[1]))
""",
                "main.py": r"""
m1 + | import sys
     |
m2 + | from inventory import parse_item
     |
     |
m3 + | def main() -> None:
m4 + |     item = parse_item(sys.argv[1])
m5 + |     print(item)
     |
     |
m6 + | if __name__ == "__main__":
m7 + |     main()
""",
                "scripts/legacy_import.py": r"""
g1 + | import sys
     |
g2 + | raw = sys.stdin.read()
g3 + | rows = [row for row in raw.split(";") if row]
g4 + | print(f"{len(rows)} legacy rows")
""",
            },
        ),
        Commit(
            message="rename inventory module to stock; replace legacy import with report",
            renames={"inventory.py": "stock.py"},
            dead="g1 g2 g3 g4",
            files={
                "stock.py": r"""
d1 ~ | # Stock rows parsed from CSV.
f1 = | from dataclasses import dataclass
f2 = | from typing import Optional
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
p5 = |     return Item(parts[0].strip(), int(parts[1]))
""",
                "main.py": r"""
m1 = | import sys
     |
m2 ~ | from stock import parse_item
     |
     |
m3 = | def main() -> None:
m4 = |     item = parse_item(sys.argv[1])
m5 = |     print(item)
     |
     |
m6 = | if __name__ == "__main__":
m7 = |     main()
""",
                "scripts/report.py": r"""
q1 + | from collections import Counter
q2 + | import sys
     |
q3 + | totals = Counter(sys.argv[1:])
q4 + | for name, count in sorted(totals.items()):
q5 + |     print(f"{name}: {count}")
""",
            },
        ),
    ],
)

TYPESCRIPT = Fixture(
    mutation_class="rename-file",
    language="typescript",
    summary=(
        "Move and rename src/inventory.ts to src/domain/stock.ts: its lines are verbatim at the new path "
        "except the header comment, which is edited; the import in main.ts is edited. In the same commit an "
        "unrelated scripts/legacyImport.ts is deleted (all lines die) and scripts/report.ts is added (all "
        "lines born); that pair shares no lines and must not be treated as a rename."
    ),
    commits=[
        Commit(
            message="add inventory parser and legacy import script",
            files={
                "src/inventory.ts": r"""
d1 + | // Inventory rows parsed from CSV.
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
p6 + |   return { name: parts[0].trim(), quantity: Number(parts[1]) };
p7 + | }
""",
                "src/main.ts": r"""
m1 + | import { parseItem } from "./inventory";
     |
m2 + | const item = parseItem(process.argv[2] ?? "");
m3 + | console.log(item);
""",
                "scripts/legacyImport.ts": r"""
g1 + | import { readFileSync } from "fs";
     |
g2 + | const raw = readFileSync(0, "utf8");
g3 + | const rows = raw.split(";").filter((row) => row !== "");
g4 + | console.log(`${rows.length} legacy rows`);
""",
            },
        ),
        Commit(
            message="move inventory module to domain/stock; replace legacy import with report",
            renames={"src/inventory.ts": "src/domain/stock.ts"},
            dead="g1 g2 g3 g4",
            files={
                "src/domain/stock.ts": r"""
d1 ~ | // Stock rows parsed from CSV.
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
p6 = |   return { name: parts[0].trim(), quantity: Number(parts[1]) };
p7 = | }
""",
                "src/main.ts": r"""
m1 ~ | import { parseItem } from "./domain/stock";
     |
m2 = | const item = parseItem(process.argv[2] ?? "");
m3 = | console.log(item);
""",
                "scripts/report.ts": r"""
q1 + | const totals = new Map<string, number>();
q2 + | for (const name of process.argv.slice(2)) {
q3 + |   totals.set(name, (totals.get(name) ?? 0) + 1);
q4 + | }
q5 + | for (const [name, count] of totals) {
q6 + |   console.log(`${name}: ${count}`);
q7 + | }
""",
            },
        ),
    ],
)

FIXTURES = [RUST, PYTHON, TYPESCRIPT]
