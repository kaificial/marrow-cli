from fixture_lib import Commit, Fixture

RUST = Fixture(
    mutation_class="rename-identifier",
    language="rust",
    summary=(
        "Rename fn parse_item to parse_record (definition, a call inside the module, and the import and "
        "call in main.rs) and local variable parts to fields. Every line containing a renamed identifier "
        "is edited in place; all other lines are verbatim."
    ),
    commits=[
        Commit(
            message="add inventory parser",
            files={
                "src/inventory.rs": r"""
u1 + | use std::collections::HashMap;
     |
s1 + | pub struct Item {
s2 + |     pub name: String,
s3 + |     pub quantity: u32,
s4 + | }
     |
d1 + | /// Parses one `name,quantity` CSV row.
p1 + | pub fn parse_item(line: &str) -> Option<Item> {
p2 + |     let mut parts = line.split(',');
p3 + |     let name = parts.next()?.trim().to_string();
p4 + |     let quantity = parts.next()?.trim().parse().ok()?;
p5 + |     Some(Item { name, quantity })
p6 + | }
     |
a1 + | pub fn parse_all(input: &str) -> Vec<Item> {
a2 + |     input.lines().filter_map(parse_item).collect()
a3 + | }
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
m1 + | mod inventory;
     |
m2 + | use inventory::parse_item;
     |
m3 + | fn main() {
m4 + |     let line = std::env::args().nth(1).unwrap_or_default();
m5 + |     match parse_item(&line) {
m6 + |         Some(item) => println!("{} x{}", item.name, item.quantity),
m7 + |         None => eprintln!("could not parse: {}", line),
m8 + |     }
m9 + | }
""",
            },
        ),
        Commit(
            message="rename parse_item to parse_record",
            files={
                "src/inventory.rs": r"""
u1 = | use std::collections::HashMap;
     |
s1 = | pub struct Item {
s2 = |     pub name: String,
s3 = |     pub quantity: u32,
s4 = | }
     |
d1 = | /// Parses one `name,quantity` CSV row.
p1 ~ | pub fn parse_record(line: &str) -> Option<Item> {
p2 ~ |     let mut fields = line.split(',');
p3 ~ |     let name = fields.next()?.trim().to_string();
p4 ~ |     let quantity = fields.next()?.trim().parse().ok()?;
p5 = |     Some(Item { name, quantity })
p6 = | }
     |
a1 = | pub fn parse_all(input: &str) -> Vec<Item> {
a2 ~ |     input.lines().filter_map(parse_record).collect()
a3 = | }
     |
i1 = | pub fn index_by_name(items: Vec<Item>) -> HashMap<String, Item> {
i2 = |     let mut index = HashMap::new();
i3 = |     for item in items {
i4 = |         index.insert(item.name.clone(), item);
i5 = |     }
i6 = |     index
i7 = | }
""",
                "src/main.rs": r"""
m1 = | mod inventory;
     |
m2 ~ | use inventory::parse_record;
     |
m3 = | fn main() {
m4 = |     let line = std::env::args().nth(1).unwrap_or_default();
m5 ~ |     match parse_record(&line) {
m6 = |         Some(item) => println!("{} x{}", item.name, item.quantity),
m7 = |         None => eprintln!("could not parse: {}", line),
m8 = |     }
m9 = | }
""",
            },
        ),
    ],
)

PYTHON = Fixture(
    mutation_class="rename-identifier",
    language="python",
    summary=(
        "Rename function parse_item to parse_record (definition, a call inside the module, and the import "
        "and call in main.py) and local variable parts to fields. Every line containing a renamed "
        "identifier is edited in place; all other lines are verbatim."
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
k1 + | @dataclass
k2 + | class Item:
k3 + |     name: str
k4 + |     quantity: int
     |
     |
d1 + | # Parses one "name,quantity" CSV row.
p1 + | def parse_item(line: str) -> Optional[Item]:
p2 + |     parts = line.split(",")
p3 + |     if len(parts) < 2:
p4 + |         return None
p5 + |     return Item(parts[0].strip(), int(parts[1]))
     |
     |
a1 + | def parse_all(text: str) -> List[Item]:
a2 + |     return [item for item in map(parse_item, text.splitlines()) if item]
     |
     |
i1 + | def index_by_name(items: List[Item]) -> Dict[str, Item]:
i2 + |     return {item.name: item for item in items}
""",
                "main.py": r"""
m1  + | import sys
      |
m2  + | from inventory import parse_item
      |
      |
m3  + | def main() -> None:
m4  + |     item = parse_item(sys.argv[1])
m5  + |     if item is None:
m6  + |         print("could not parse", file=sys.stderr)
m7  + |     else:
m8  + |         print(f"{item.name} x{item.quantity}")
      |
      |
m9  + | if __name__ == "__main__":
m10 + |     main()
""",
            },
        ),
        Commit(
            message="rename parse_item to parse_record",
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
d1 = | # Parses one "name,quantity" CSV row.
p1 ~ | def parse_record(line: str) -> Optional[Item]:
p2 ~ |     fields = line.split(",")
p3 ~ |     if len(fields) < 2:
p4 = |         return None
p5 ~ |     return Item(fields[0].strip(), int(fields[1]))
     |
     |
a1 = | def parse_all(text: str) -> List[Item]:
a2 ~ |     return [item for item in map(parse_record, text.splitlines()) if item]
     |
     |
i1 = | def index_by_name(items: List[Item]) -> Dict[str, Item]:
i2 = |     return {item.name: item for item in items}
""",
                "main.py": r"""
m1  = | import sys
      |
m2  ~ | from inventory import parse_record
      |
      |
m3  = | def main() -> None:
m4  ~ |     item = parse_record(sys.argv[1])
m5  = |     if item is None:
m6  = |         print("could not parse", file=sys.stderr)
m7  = |     else:
m8  = |         print(f"{item.name} x{item.quantity}")
      |
      |
m9  = | if __name__ == "__main__":
m10 = |     main()
""",
            },
        ),
    ],
)

TYPESCRIPT = Fixture(
    mutation_class="rename-identifier",
    language="typescript",
    summary=(
        "Rename function parseItem to parseRecord (definition, a reference inside the module, and the "
        "import and call in main.ts) and local variable parts to fields. Every line containing a renamed "
        "identifier is edited in place; all other lines are verbatim."
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
d1 + | // Parses one "name,quantity" CSV row.
p1 + | export function parseItem(line: string): Item | undefined {
p2 + |   const parts = line.split(",");
p3 + |   if (parts.length < 2) {
p4 + |     return undefined;
p5 + |   }
p6 + |   return { name: parts[0].trim(), quantity: Number(parts[1]) };
p7 + | }
     |
a1 + | export function parseAll(text: string): Item[] {
a2 + |   return text
a3 + |     .split("\n")
a4 + |     .map(parseItem)
a5 + |     .filter((item): item is Item => item !== undefined);
a6 + | }
     |
i1 + | export function indexByName(items: Item[]): Map<string, Item> {
i2 + |   return new Map(items.map((item) => [item.name, item]));
i3 + | }
""",
                "src/main.ts": r"""
m1 + | import { parseItem } from "./inventory";
     |
m2 + | const line = process.argv[2] ?? "";
m3 + | const item = parseItem(line);
m4 + | if (item === undefined) {
m5 + |   console.error(`could not parse: ${line}`);
m6 + | } else {
m7 + |   console.log(`${item.name} x${item.quantity}`);
m8 + | }
""",
            },
        ),
        Commit(
            message="rename parseItem to parseRecord",
            files={
                "src/inventory.ts": r"""
s1 = | export interface Item {
s2 = |   name: string;
s3 = |   quantity: number;
s4 = | }
     |
d1 = | // Parses one "name,quantity" CSV row.
p1 ~ | export function parseRecord(line: string): Item | undefined {
p2 ~ |   const fields = line.split(",");
p3 ~ |   if (fields.length < 2) {
p4 = |     return undefined;
p5 = |   }
p6 ~ |   return { name: fields[0].trim(), quantity: Number(fields[1]) };
p7 = | }
     |
a1 = | export function parseAll(text: string): Item[] {
a2 = |   return text
a3 = |     .split("\n")
a4 ~ |     .map(parseRecord)
a5 = |     .filter((item): item is Item => item !== undefined);
a6 = | }
     |
i1 = | export function indexByName(items: Item[]): Map<string, Item> {
i2 = |   return new Map(items.map((item) => [item.name, item]));
i3 = | }
""",
                "src/main.ts": r"""
m1 ~ | import { parseRecord } from "./inventory";
     |
m2 = | const line = process.argv[2] ?? "";
m3 ~ | const item = parseRecord(line);
m4 = | if (item === undefined) {
m5 = |   console.error(`could not parse: ${line}`);
m6 = | } else {
m7 = |   console.log(`${item.name} x${item.quantity}`);
m8 = | }
""",
            },
        ),
    ],
)

FIXTURES = [RUST, PYTHON, TYPESCRIPT]
