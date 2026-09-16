from fixture_lib import UNCHANGED, Commit, Fixture

RUST = Fixture(
    mutation_class="full-rewrite",
    language="rust",
    summary=(
        "Replace src/inventory.rs wholesale with a different design (FromStr Record plus an Inventory "
        "type). Every old line dies and every new line is born, including new lines whose text exactly "
        "matches an old one: `use std::collections::HashMap;` and several `}` lines. src/money.rs is "
        "untouched."
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
p1 + | pub fn parse_item(line: &str) -> Option<Item> {
p2 + |     let mut parts = line.split(',');
p3 + |     let name = parts.next()?.trim().to_string();
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
                "src/money.rs": r"""
g1 + | pub fn format_cents(cents: u64) -> String {
g2 + |     format!("${}.{:02}", cents / 100, cents % 100)
g3 + | }
""",
            },
        ),
        Commit(
            message="rewrite inventory around Record and Inventory",
            dead="u1 s1 s2 s3 s4 p1 p2 p3 p4 p5 p6 i1 i2 i3 i4 i5 i6 i7",
            files={
                "src/inventory.rs": r"""
x1  + | use std::collections::HashMap;
x2  + | use std::str::FromStr;
      |
x3  + | #[derive(Debug, Clone)]
x4  + | pub struct Record {
x5  + |     pub sku: String,
x6  + |     pub count: u32,
x7  + | }
      |
x8  + | impl FromStr for Record {
x9  + |     type Err = String;
      |
x10 + |     fn from_str(s: &str) -> Result<Self, Self::Err> {
x11 + |         let (sku, count) = s.split_once(',').ok_or("missing comma")?;
x12 + |         let count = count.trim().parse().map_err(|_| "bad count")?;
x13 + |         Ok(Record { sku: sku.trim().to_owned(), count })
x14 + |     }
x15 + | }
      |
x16 + | pub struct Inventory {
x17 + |     records: HashMap<String, Record>,
x18 + | }
      |
x19 + | impl Inventory {
x20 + |     pub fn load(text: &str) -> Result<Self, String> {
x21 + |         let records = text
x22 + |             .lines()
x23 + |             .map(Record::from_str)
x24 + |             .map(|r| r.map(|rec| (rec.sku.clone(), rec)))
x25 + |             .collect::<Result<_, _>>()?;
x26 + |         Ok(Inventory { records })
x27 + |     }
x28 + | }
""",
                "src/money.rs": UNCHANGED,
            },
        ),
    ],
)

PYTHON = Fixture(
    mutation_class="full-rewrite",
    language="python",
    summary=(
        "Replace inventory.py wholesale with a different design (frozen Record dataclass plus an Inventory "
        "container). Every old line dies and every new line is born, including the new line "
        "`from typing import Dict, List, Optional`, which exactly matches an old import. money.py is "
        "untouched."
    ),
    commits=[
        Commit(
            message="add inventory parser",
            files={
                "inventory.py": r"""
u1 + | from typing import Dict, List, Optional
     |
     |
k1 + | class Item:
k2 + |     def __init__(self, name: str, quantity: int):
k3 + |         self.name = name
k4 + |         self.quantity = quantity
     |
     |
p1 + | def parse_item(line: str) -> Optional[Item]:
p2 + |     parts = line.split(",")
p3 + |     if len(parts) < 2:
p4 + |         return None
p5 + |     return Item(parts[0].strip(), int(parts[1]))
     |
     |
i1 + | def index_by_name(items: List[Item]) -> Dict[str, Item]:
i2 + |     index = {}
i3 + |     for item in items:
i4 + |         index[item.name] = item
i5 + |     return index
""",
                "money.py": r"""
g1 + | def format_cents(cents: int) -> str:
g2 + |     return f"${cents // 100}.{cents % 100:02d}"
""",
            },
        ),
        Commit(
            message="rewrite inventory around Record and Inventory",
            dead="u1 k1 k2 k3 k4 p1 p2 p3 p4 p5 i1 i2 i3 i4 i5",
            files={
                "inventory.py": r"""
x1  + | from dataclasses import dataclass, field
x2  + | from typing import Dict, List, Optional
      |
      |
x3  + | @dataclass(frozen=True)
x4  + | class Record:
x5  + |     sku: str
x6  + |     count: int
      |
x7  + |     @classmethod
x8  + |     def from_csv(cls, row: str) -> "Record":
x9  + |         sku, _, count = row.partition(",")
x10 + |         if not count:
x11 + |             raise ValueError(f"missing count in {row!r}")
x12 + |         return cls(sku.strip(), int(count))
      |
      |
x13 + | @dataclass
x14 + | class Inventory:
x15 + |     records: Dict[str, Record] = field(default_factory=dict)
      |
x16 + |     def load(self, rows: List[str]) -> None:
x17 + |         for row in rows:
x18 + |             record = Record.from_csv(row)
x19 + |             self.records[record.sku] = record
      |
x20 + |     def get(self, sku: str) -> Optional[Record]:
x21 + |         return self.records.get(sku)
""",
                "money.py": UNCHANGED,
            },
        ),
    ],
)

TYPESCRIPT = Fixture(
    mutation_class="full-rewrite",
    language="typescript",
    summary=(
        "Replace src/inventory.ts wholesale with a different design (Record class plus an Inventory "
        "class). Every old line dies and every new line is born, including new `}` and `  }` lines whose "
        "text exactly matches old ones. src/money.ts is untouched."
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
p6 + |   return { name: parts[0].trim(), quantity: Number(parts[1]) };
p7 + | }
     |
i1 + | export function indexByName(items: Item[]): Map<string, Item> {
i2 + |   const index = new Map<string, Item>();
i3 + |   for (const item of items) {
i4 + |     index.set(item.name, item);
i5 + |   }
i6 + |   return index;
i7 + | }
""",
                "src/money.ts": r"""
g1 + | export function formatCents(cents: number): string {
g2 + |   return `$${Math.floor(cents / 100)}.${String(cents % 100).padStart(2, "0")}`;
g3 + | }
""",
            },
        ),
        Commit(
            message="rewrite inventory around Record and Inventory",
            dead="s1 s2 s3 s4 p1 p2 p3 p4 p5 p6 p7 i1 i2 i3 i4 i5 i6 i7",
            files={
                "src/inventory.ts": r"""
x1  + | export class Record {
x2  + |   constructor(
x3  + |     readonly sku: string,
x4  + |     readonly count: number,
x5  + |   ) {}
      |
x6  + |   static fromCsv(row: string): Record {
x7  + |     const [sku, count] = row.split(",", 2);
x8  + |     if (count === undefined) {
x9  + |       throw new Error(`missing count in ${row}`);
x10 + |     }
x11 + |     return new Record(sku.trim(), Number.parseInt(count, 10));
x12 + |   }
x13 + | }
      |
x14 + | export class Inventory {
x15 + |   private readonly records = new Map<string, Record>();
      |
x16 + |   load(rows: string[]): void {
x17 + |     for (const row of rows) {
x18 + |       const record = Record.fromCsv(row);
x19 + |       this.records.set(record.sku, record);
x20 + |     }
x21 + |   }
      |
x22 + |   get(sku: string): Record | undefined {
x23 + |     return this.records.get(sku);
x24 + |   }
x25 + | }
""",
                "src/money.ts": UNCHANGED,
            },
        ),
    ],
)

FIXTURES = [RUST, PYTHON, TYPESCRIPT]
