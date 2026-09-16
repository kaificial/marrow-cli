from fixture_lib import Commit, Fixture

RUST = Fixture(
    mutation_class="format-only",
    language="rust",
    summary=(
        "rustfmt applied to hand-spaced code: operator and brace spacing, indent width 2 to 4, "
        "a double blank line collapsed. No line splits, joins, token changes, or reordering."
    ),
    commits=[
        Commit(
            message="add inventory parser",
            files={
                "src/inventory.rs": r"""
u1  + | use std::collections::HashMap;
cm1 + | // Parsed inventory row.
s1  + | pub struct Item{
s2  + |   pub name:String,
s3  + |   pub quantity:u32,
s4  + | }
p1  + | pub fn parse_item(line:&str)->Option<Item>{
p2  + |   let mut parts=line.split(',');
p3  + |   let name=parts.next()?.trim().to_string();
p4  + |   let quantity=parts.next()?.trim().parse().ok()?;
p5  + |   Some(Item{name,quantity})
p6  + | }
      |
      |
i1  + | pub fn index_by_name(items:Vec<Item>)->HashMap<String,Item>{
i2  + |   let mut index=HashMap::new();
i3  + |   for item in items{
i4  + |     index.insert(item.name.clone(),item);
i5  + |   }
i6  + |   index
i7  + | }
""",
            },
        ),
        Commit(
            message="run rustfmt",
            files={
                "src/inventory.rs": r"""
u1  = | use std::collections::HashMap;
cm1 = | // Parsed inventory row.
s1  = | pub struct Item {
s2  = |     pub name: String,
s3  = |     pub quantity: u32,
s4  = | }
p1  = | pub fn parse_item(line: &str) -> Option<Item> {
p2  = |     let mut parts = line.split(',');
p3  = |     let name = parts.next()?.trim().to_string();
p4  = |     let quantity = parts.next()?.trim().parse().ok()?;
p5  = |     Some(Item { name, quantity })
p6  = | }
      |
i1  = | pub fn index_by_name(items: Vec<Item>) -> HashMap<String, Item> {
i2  = |     let mut index = HashMap::new();
i3  = |     for item in items {
i4  = |         index.insert(item.name.clone(), item);
i5  = |     }
i6  = |     index
i7  = | }
""",
            },
        ),
    ],
)

PYTHON = Fixture(
    mutation_class="format-only",
    language="python",
    summary=(
        "black applied to hand-spaced code: operator and annotation spacing, indent width 2 to 4, "
        "blank lines inserted between top-level definitions and a triple blank collapsed. "
        "No line splits, joins, quote changes, or trailing commas."
    ),
    commits=[
        Commit(
            message="add inventory parser",
            files={
                "inventory.py": r"""
t1  + | from typing import Dict,List,Optional
cm1 + | # Parsed inventory row.
k1  + | class Item:
k2  + |   def __init__(self,name:str,quantity:int):
k3  + |     self.name=name
k4  + |     self.quantity=quantity
p1  + | def parse_item(line:str)->Optional[Item]:
p2  + |   parts=line.split(",")
p3  + |   if len(parts)<2:
p4  + |     return None
p5  + |   return Item(parts[0].strip(),int(parts[1]))
      |
      |
      |
i1  + | def index_by_name(items:List[Item])->Dict[str,Item]:
i2  + |   index={}
i3  + |   for item in items:
i4  + |     index[item.name]=item
i5  + |   return index
""",
            },
        ),
        Commit(
            message="run black",
            files={
                "inventory.py": r"""
t1  = | from typing import Dict, List, Optional
      |
      |
cm1 = | # Parsed inventory row.
k1  = | class Item:
k2  = |     def __init__(self, name: str, quantity: int):
k3  = |         self.name = name
k4  = |         self.quantity = quantity
      |
      |
p1  = | def parse_item(line: str) -> Optional[Item]:
p2  = |     parts = line.split(",")
p3  = |     if len(parts) < 2:
p4  = |         return None
p5  = |     return Item(parts[0].strip(), int(parts[1]))
      |
      |
i1  = | def index_by_name(items: List[Item]) -> Dict[str, Item]:
i2  = |     index = {}
i3  = |     for item in items:
i4  = |         index[item.name] = item
i5  = |     return index
""",
            },
        ),
    ],
)

TYPESCRIPT = Fixture(
    mutation_class="format-only",
    language="typescript",
    summary=(
        "prettier applied to hand-spaced code: brace, colon, and operator spacing, indent width 4 to 2, "
        "a triple blank line collapsed. No line splits, joins, quote or semicolon changes."
    ),
    commits=[
        Commit(
            message="add inventory parser",
            files={
                "src/inventory.ts": r"""
cm1 + | // Parsed inventory row.
s1  + | export interface Item{
s2  + |     name:string;
s3  + |     quantity:number;
s4  + | }
p1  + | export function parseItem(line:string):Item|undefined{
p2  + |     const parts=line.split(",");
p3  + |     if(parts.length<2){
p4  + |         return undefined;
p5  + |     }
p6  + |     return {name:parts[0].trim(),quantity:Number(parts[1])};
p7  + | }
      |
      |
      |
i1  + | export function indexByName(items:Item[]):Map<string,Item>{
i2  + |     const index=new Map<string,Item>();
i3  + |     for(const item of items){
i4  + |         index.set(item.name,item);
i5  + |     }
i6  + |     return index;
i7  + | }
""",
            },
        ),
        Commit(
            message="run prettier",
            files={
                "src/inventory.ts": r"""
cm1 = | // Parsed inventory row.
s1  = | export interface Item {
s2  = |   name: string;
s3  = |   quantity: number;
s4  = | }
p1  = | export function parseItem(line: string): Item | undefined {
p2  = |   const parts = line.split(",");
p3  = |   if (parts.length < 2) {
p4  = |     return undefined;
p5  = |   }
p6  = |   return { name: parts[0].trim(), quantity: Number(parts[1]) };
p7  = | }
      |
i1  = | export function indexByName(items: Item[]): Map<string, Item> {
i2  = |   const index = new Map<string, Item>();
i3  = |   for (const item of items) {
i4  = |     index.set(item.name, item);
i5  = |   }
i6  = |   return index;
i7  = | }
""",
            },
        ),
    ],
)

FIXTURES = [RUST, PYTHON, TYPESCRIPT]
