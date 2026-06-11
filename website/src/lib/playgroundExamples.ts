export type PlaygroundExampleId =
  | 'patternMatching'
  | 'structTrait'
  | 'namedParams'
  | 'ranges'
  | 'templateStrings'
  | 'errorHandling'
  | 'closures'
  | 'configParser'
  | 'sortSearch'
  | 'listIterSugar'
  | 'listOps'
  | 'jsonProcess'
  | 'macros'

export type PlaygroundSelectionId = PlaygroundExampleId | 'custom'

export type PlaygroundExample = {
  id: PlaygroundExampleId
  sourcePath: string
  code: string
}

export const playgroundExamples: PlaygroundExample[] = [
  {
    id: 'patternMatching',
    sourcePath: 'examples/syntax/pattern_matching.lk',
    code: `// Pattern matching with if let / while let / let destructuring

// 1. let with simple list destructuring
let [a, b] = [10, 20];
assert(a == 10);
assert(b == 20);

// 2. let with list rest
let [first, second, ..rest] = [1, 2, 3, 4, 5];
assert(first == 1);
assert(second == 2);
assert(rest == [3, 4, 5]);

// 3. let with map destructuring (string keys in pattern)
let user = { "name": "Bob", "age": 25, "city": "NYC" };
let { "name": n, "age": a } = user;
assert(n == "Bob");
assert(a == 25);

// 4. let with map rest
let { "name": who, ..remaining } = user;
assert(who == "Bob");
assert(remaining.has("age"));
assert(remaining.has("city"));

// 5. if let — conditional destructuring
let maybe_num = [1, 2, 3];
if let [head, ..tail] = maybe_num {
  assert(head == 1);
  assert(tail == [2, 3]);
} else {
  panic("if let should match");
}

// 6. if let with literal — only matches that literal
let val = 42;
if let 42 = val {
  assert(true);
} else {
  panic("if let 42 should match");
}

// 7. while let — loop until pattern fails
let stack = [1, 2, 3];
let collected = [];
while let [top, ..rest] = stack {
  collected.push(top);
  stack = rest;
}
assert(collected == [1, 2, 3]);

// 8. Nested destructuring
let data = { "point": [10, 20] };
let { "point": [px, py] } = data;
assert(px == 10);
assert(py == 20);

// 9. Wildcard in destructuring
let [_, second_item, .._] = [100, 200, 300, 400];
assert(second_item == 200);

println("pattern_matching: all assertions passed");`,
  },
  {
    id: 'structTrait',
    sourcePath: 'examples/syntax/struct_trait.lk',
    code: `// Struct, Trait, and impl demo

// 1. Define structs
struct Point { x: Int, y: Int }
struct Rect { w: Int, h: Int }
struct Circle { r: Int }

// 2. Instantiate structs
let p = Point { x: 3, y: 4 };
assert(p.x == 3);
assert(p.y == 4);

let r = Rect { w: 10, h: 20 };
assert(r.w == 10);

let c = Circle { r: 5 };
assert(c.r == 5);

// 3. Struct with optional field
struct User { id: Int, name: String?, active: Bool }
let u1 = User { id: 1, name: "Alice", active: true };
let u2 = User { id: 2, active: false };
assert(u1.name == "Alice");
assert(u2.name == nil);
assert(u2.active == false);

// 4. Trait definition and implementation
trait Area {
  fn area(self) -> Int;
}

impl Area for Rect {
  fn area(self) -> Int { return self.w * self.h; }
}

assert(r.area() == 200);

// 5. Multiple traits
trait Describe {
  fn describe(self) -> String;
}

impl Describe for Rect {
  fn describe(self) -> String { return "Rect(\${self.w}x\${self.h})"; }
}
assert(r.describe() == "Rect(10x20)");

// 6. Same trait, different types
impl Area for Circle {
  fn area(self) -> Int { return 3 * self.r * self.r; } // approximate
}
let c_area = c.area();
assert(c_area == 75);

impl Describe for Circle {
  fn describe(self) -> String { return "Circle(r=\${self.r})"; }
}
assert(c.describe() == "Circle(r=5)");

// 7. Struct in collections
let shapes = [Rect { w: 3, h: 4 }, Circle { r: 10 }];
let areas = shapes.map(|s| s.area());
assert(areas == [12, 300]);

println("struct_trait: all assertions passed");`,
  },
  {
    id: 'namedParams',
    sourcePath: 'examples/syntax/named_params.lk',
    code: `// Named parameter examples

fn draw_rect(x: Int, y: Int, {w: Int, h: Int? = 100}) -> Int {
    // pretend to construct a rect and return area
    // Note: 'if' is a statement, not an expression. Use nullish coalescing here.
    let height: Int = h ?? 0;
    return w * height;
}

fn configure({host: String, timeout_ms: Int? = 1000}) {
    println("Connecting to {} with timeout {}ms", host, timeout_ms);
}

let a = draw_rect(10, 20, w: 300, h: 200);
let b = draw_rect(5, 5, h: 50, w: 60);

configure(host: "example.com");`,
  },
  {
    id: 'ranges',
    sourcePath: 'examples/syntax/ranges.lk',
    code: `// Ranges and iteration demo

use iter;

// 1. Exclusive range a..b produces a list
let r1 = 1..5;
assert(r1 == [1, 2, 3, 4]);

// 2. Inclusive range a..=b produces a list
let r2 = 1..=5;
assert(r2 == [1, 2, 3, 4, 5]);

// 3. Range in for loop
let sum = 0;
for i in 1..=10 { sum += i; }
assert(sum == 55);

// 4. iter.range with step
let evens = iter.range(0, 10, 2);
assert(evens == [0, 2, 4, 6, 8]);

// 5. iter.range with default step
let nums = iter.range(3, 7);
assert(nums == [3, 4, 5, 6]);

// 6. Ranges as list slicing helpers
let xs = [10, 20, 30, 40, 50];
assert(xs.take(3) == [10, 20, 30]);
assert(xs.skip(2) == [30, 40, 50]);

// 7. Chaining (both operands must be lists; ranges auto-produce lists)
let left = 1..=3;
let right = [10, 11, 12];
let chained = left.chain(right);
assert(chained == [1, 2, 3, 10, 11, 12]);

// 8. in operator with ranges (use variable to avoid inline evaluation bug)
let range_list = 1..=5;
assert(3 in range_list);

println("ranges: all assertions passed");`,
  },
  {
    id: 'templateStrings',
    sourcePath: 'examples/syntax/template_strings.lk',
    code: `// Template strings and string formatting demo

// 1. Basic interpolation
let name = "LK";
let s1 = "Hello, \${name}!";
assert(s1 == "Hello, LK!");

// 2. Expression interpolation
let x = 10;
let y = 20;
let s2 = "\${x} + \${y} = \${x + y}";
assert(s2 == "10 + 20 = 30");

// 3. Nested property access
let user = { "name": "Alice", "address": { "city": "NYC" } };
let s3 = "User \${user.name} from \${user.address.city}";
assert(s3 == "User Alice from NYC");

// 4. Method calls in interpolation
let items = [1, 2, 3];
let s4 = "Count: \${items.len()}";
assert(s4 == "Count: 3");

// 5. Function calls in interpolation
fn double(n) { return n * 2; }
let s5 = "Double of 5 is \${double(5)}";
assert(s5 == "Double of 5 is 10");

// 6. Single-quoted strings also support interpolation
let s6 = 'Value: \${x}';
assert(s6 == "Value: 10");

// 7. Raw strings do NOT interpolate
// Comparison must also use raw string, or \${x} in the normal string gets interpolated
let raw = r"No \${x} interpolation";
assert(raw == r"No \${x} interpolation");

// 8. println with format placeholders
let pi = 3.14159;
println("pi = {}", pi);

// 9. Multiple format args
let a = 1;
let b = 2;
println("{} + {} = {}", a, b, a + b);

// 10. Escaping $ in template strings
let s10 = "Price: \\$100";
assert(s10 == "Price: $100");

println("template_strings: all assertions passed");`,
  },
  {
    id: 'errorHandling',
    sourcePath: 'examples/syntax/error_handling.lk',
    code: `// Error handling patterns in LK
// LK uses nil to signal absence and ?? for defaults

// 1. Return nil on failure
fn safe_divide(a, b) {
  if (b == 0) { return nil; }
  return a / b;
}
assert(safe_divide(10, 2) > 4.9);
assert(safe_divide(10, 0) == nil);

// 2. Use ?? for default values when operation fails
let result = safe_divide(10, 0) ?? 0;
assert(result == 0);

// 3. Chain of operations with nil propagation
fn get_nested(data, key1, key2) {
  let level1 = data.get(key1);
  if (level1 == nil) { return nil; }
  return level1.get(key2);
}
let obj = { "user": { "email": "alice@example.com" } };
assert(get_nested(obj, "user", "email") == "alice@example.com");
assert(get_nested(obj, "admin", "email") == nil);
let email = get_nested(obj, "admin", "email") ?? "no-email";
assert(email == "no-email");

// 4. Validation using ?? with defaults
fn validate_name(name) {
  return name ?? "name required";
}
assert(validate_name("Alice") == "Alice");
assert(validate_name(nil) == "name required");

// 5. Result list — [ok, value_or_error] pair
fn safe_lookup(map, key) {
  let val = map.get(key);
  if (val == nil) { return [false, "key not found"]; }
  return [true, val];
}
let config = { "host": "localhost", "port": 8080 };
let [ok1, val1] = safe_lookup(config, "host");
assert(ok1 == true);
assert(val1 == "localhost");
let [ok2, val2] = safe_lookup(config, "ssl");
assert(ok2 == false);
assert(val2 == "key not found");

// 6. Range check — returns error string or nil
fn check_range(n, lo, hi, label) {
  if (n < lo) { return "\${label} too low (min \${lo})"; }
  if (n > hi) { return "\${label} too high (max \${hi})"; }
  return nil;
}
assert(check_range(5, 0, 10, "age") == nil);
assert(check_range(-1, 0, 10, "age") == "age too low (min 0)");
assert(check_range(99, 0, 10, "age") == "age too high (max 10)");

// 7. Multiple lookups with ?? chaining
let db = { "primary": nil, "secondary": "fallback-value" };
let primary_val = db.get("primary");
let secondary_val = db.get("secondary");
let final_val = primary_val ?? secondary_val ?? "default";
assert(final_val == "fallback-value");

println("error_handling: all assertions passed");`,
  },
  {
    id: 'closures',
    sourcePath: 'examples/syntax/closure.lk',
    code: `// Closure and higher-order function demo

// 1. Basic closures
let double = |x| x * 2;
let square = |x| x * x;
assert(double(5) == 10);
assert(square(4) == 16);

// 2. Multi-parameter closure
let add = |a, b| a + b;
assert(add(3, 7) == 10);

// 3. Closures capture enclosing variables
let factor = 3;
let scale = |x| x * factor;
assert(scale(4) == 12);

// 4. Closures as arguments (higher-order functions)
let apply = |f, x| f(x);
assert(apply(double, 7) == 14);
assert(apply(square, 5) == 25);

// 5. Returning closures from functions
fn multiplier(n) {
  return |x| x * n;
}
let triple = multiplier(3);
let quintuple = multiplier(5);
assert(triple(4) == 12);
assert(quintuple(4) == 20);

// 6. Closure with list methods
let nums = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
let evens = nums.filter(|x| x % 2 == 0);
let doubled = nums.map(|x| x * 2);
let total = nums.reduce(0, |acc, x| acc + x);
assert(evens == [2, 4, 6, 8, 10]);
assert(doubled == [2, 4, 6, 8, 10, 12, 14, 16, 18, 20]);
assert(total == 55);

// 7. Composing operations with a helper function
fn process_list(xs) {
  let filtered = xs.filter(|x| x > 3);
  let mapped = filtered.map(|x| x * x);
  return mapped.reduce(0, |a, b| a + b);
}
assert(process_list([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]) == 371);

println("closure: all assertions passed");`,
  },
  {
    id: 'configParser',
    sourcePath: 'examples/general/config_parser.lk',
    code: `// Config file parsing demo — combine JSON, YAML, TOML

use { json, yaml, toml } from encoding;

// Simulate reading different config file formats

// 1. JSON config
let json_cfg = "{ \\"host\\": \\"db.example.com\\", \\"port\\": 5432, \\"pool_size\\": 10 }";
let j = json.parse(json_cfg);
assert(j.host == "db.example.com");
assert(j.port == 5432);

// 2. YAML config
let yaml_cfg = "logging:\\n  level: debug\\n  file: /var/log/app.log\\n";
let y = yaml.parse(yaml_cfg);
assert(y.logging.level == "debug");
assert(y.logging.file == "/var/log/app.log");

// 3. TOML config
let toml_cfg = "[server]\\nhost = \\"0.0.0.0\\"\\nport = 8080\\n\\n[ssl]\\nenabled = true\\ncert = \\"/etc/ssl/cert.pem\\"\\n";
let t = toml.parse(toml_cfg);
assert(t.server.host == "0.0.0.0");
assert(t.server.port == 8080);
assert(t.ssl.enabled == true);
assert(t.ssl.cert == "/etc/ssl/cert.pem");

// 4. Merge configs into a unified structure
fn build_dsn(db_cfg) {
  return "\${db_cfg.host}:\${db_cfg.port}";
}
let dsn = build_dsn(j);
assert(dsn == "db.example.com:5432");

// 5. Feature flag check from TOML
fn is_feature_enabled(cfg, feature) {
  if (!cfg.ssl.has(feature)) { return false; }
  return cfg.ssl[feature] == true;
}
assert(is_feature_enabled(t, "enabled"));

println("config_parser: all assertions passed");`,
  },
  {
    id: 'sortSearch',
    sourcePath: 'examples/general/sort_search.lk',
    code: `// Sorting and searching algorithms demo

// 1. Insertion sort (using reduce to avoid if-block scoping issues)
fn insert_sorted(sorted, item) {
  let i = 0;
  while (i < sorted.len() && sorted[i] < item) {
    i += 1;
  }
  return sorted.take(i).concat([item]).concat(sorted.skip(i));
}

fn insertion_sort(xs) {
  return xs.reduce([], |sorted, item| insert_sorted(sorted, item));
}

let unsorted = [5, 3, 8, 1, 9, 2, 7, 4, 6];
let sorted = insertion_sort(unsorted);
assert(sorted == [1, 2, 3, 4, 5, 6, 7, 8, 9]);

// 2. Sort descending — reverse the result
fn reverse_list(xs) {
  return xs.reduce([], |acc, x| [x].concat(acc));
}
let desc = reverse_list(sorted);
assert(desc == [9, 8, 7, 6, 5, 4, 3, 2, 1]);

// 3. Sort strings
let words = ["banana", "apple", "cherry", "date"];
let sorted_words = insertion_sort(words);
assert(sorted_words == ["apple", "banana", "cherry", "date"]);

// 4. Linear search
fn linear_search(xs, target) {
  let i = 0;
  while (i < xs.len()) {
    if (xs[i] == target) { return i; }
    i += 1;
  }
  return -1;
}
assert(linear_search(sorted, 5) == 4);
assert(linear_search(sorted, 99) == -1);

// 5. Min/max finder using reduce
fn my_min(a, b) {
  if (a < b) { return a; }
  return b;
}
fn my_max(a, b) {
  if (a > b) { return a; }
  return b;
}
let min_val = unsorted.reduce(unsorted[0], |a, b| my_min(a, b));
let max_val = unsorted.reduce(unsorted[0], |a, b| my_max(a, b));
assert(min_val == 1);
assert(max_val == 9);

// 6. Remove duplicates + sort
let with_dupes = [3, 1, 4, 1, 5, 9, 2, 6, 5, 3, 5];
let sorted_unique = insertion_sort(with_dupes.unique());
assert(sorted_unique == [1, 2, 3, 4, 5, 6, 9]);

// 7. Merge sort style: merge two sorted lists
fn merge(a, b) {
  let result = [];
  let i = 0;
  let j = 0;
  while (i < a.len() && j < b.len()) {
    if (a[i] <= b[j]) {
      result.push(a[i]);
      i += 1;
    } else {
      result.push(b[j]);
      j += 1;
    }
  }
  // Append remaining
  while (i < a.len()) {
    result.push(a[i]);
    i += 1;
  }
  while (j < b.len()) {
    result.push(b[j]);
    j += 1;
  }
  return result;
}
let merged = merge([1, 3, 5], [2, 4, 6]);
assert(merged == [1, 2, 3, 4, 5, 6]);

println("sort_search: all assertions passed");`,
  },
  {
    id: 'listIterSugar',
    sourcePath: 'examples/stdlib/list_iter_sugar.lk',
    code: `// Examples for list/iter interop through module-level APIs

use iter;

// Basic higher-order ops
let xs = [1,2,3,4,5];
let ys = iter.map(xs, |x| x * 2);
let zs = iter.filter(xs, |x| x % 2 == 0);
let sum = iter.reduce(xs, 0, |acc, x| acc + x);

println("ys={}", ys);                      // [2,4,6,8,10]
println("zs={}", zs);                      // [2,4]
println("sum={}", sum);                    // 15

// Iterator-style helpers as iter module functions
println("take={}", iter.take(xs, 3));             // [1,2,3]
println("skip={}", iter.skip(xs, 3));             // [4,5]
println("chain={}", iter.chain(iter.take(xs, 2), [9,9])); // [1,2,9,9]
let nested_left = [1, 2];
let nested_right = [3];
println("flatten={}", iter.flatten([nested_left, nested_right, 4])); // [1,2,3,4]
println("unique={}", iter.unique([1,2,1,3,2]));     // [1,2,3]

let chunks = iter.chunk(xs, 2);
println("chunks_len={}", chunks.len());          // 3

println("enumerate={}", iter.enumerate(xs));    // [[0,1],[1,2],[2,3],[3,4],[4,5]]
println("zip={}", iter.zip([1,2], ["a","b","c"])); // [[1,"a"],[2,"b"]]

return nil;`,
  },
  {
    id: 'listOps',
    sourcePath: 'examples/stdlib/list_ops.lk',
    code: `// List operations deep dive

// 1. Creation and indexing
let xs = [10, 20, 30, 40, 50];
assert(xs[0] == 10);
assert(xs[4] == 50);

// 2. first / last
assert(xs.first() == 10);
assert(xs.last() == 50);

// 3. len / push
let mutable = [1, 2];
mutable.push(3);
assert(mutable.len() == 3);
assert(mutable == [1, 2, 3]);

// 4. concat — join two lists into a new one
let joined = [1, 2].concat([3, 4]);
assert(joined == [1, 2, 3, 4]);

// 5. join — turn list into a string
let csv = ["a", "b", "c"].join(",");
assert(csv == "a,b,c");

// 6. get — safe index access (returns nil for out of bounds)
let safe = xs.get(2);
assert(safe == 30);
assert(xs.get(100) == nil);

// 7. Heterogeneous lists
let mixed = [1, "two", true, nil, [4, 5]];
assert(mixed.len() == 5);
assert(mixed[1] == "two");
assert(mixed[4] == [4, 5]);

// 8. List comprehension via map/filter
let result = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
  .filter(|x| x % 3 == 0)
  .map(|x| x * x);
assert(result == [9, 36, 81]);

// 9. Chunk — split into groups
let chunks = [1, 2, 3, 4, 5, 6, 7].chunk(2);
assert(chunks.len() == 4);

// 10. Enumerate and zip
let letters = ["a", "b", "c"];
let indexed = letters.enumerate();
assert(indexed == [[0, "a"], [1, "b"], [2, "c"]]);

let zipped = [1, 2, 3].zip(["x", "y", "z"]);
assert(zipped == [[1, "x"], [2, "y"], [3, "z"]]);

// 11. Unique
assert([1, 2, 1, 3, 2, 4].unique() == [1, 2, 3, 4]);

// 12. Flatten
assert([[1, 2], [3], [4, 5, 6]].flatten() == [1, 2, 3, 4, 5, 6]);

println("list_ops: all assertions passed");`,
  },
  {
    id: 'jsonProcess',
    sourcePath: 'examples/stdlib/json_process.lk',
    code: `// Practical JSON data processing demo

use { json } from encoding;

// Simulate receiving a JSON payload (single-line string)
let payload = "{ \\"users\\": [{ \\"id\\": 1, \\"name\\": \\"Alice\\", \\"scores\\": [95, 87, 92] }, { \\"id\\": 2, \\"name\\": \\"Bob\\", \\"scores\\": [78, 85, 90] }, { \\"id\\": 3, \\"name\\": \\"Carol\\", \\"scores\\": [100, 95, 98] }]}";

// Parse the JSON
let data = json.parse(payload);
assert(data.users.len() == 3);

// 1. Find all user names
let names = data.users.map(|u| u.name);
assert(names == ["Alice", "Bob", "Carol"]);

// 2. Compute average score per user
fn avg_score(user) {
  return user.scores.reduce(0, |a, b| a + b) / user.scores.len();
}
let averages = data.users.map(|u| avg_score(u));
assert(averages[0] > 90);
assert(averages[2] >= 97);

// 3. Filter users with average > 90
let top_users = data.users.filter(|u| avg_score(u) > 90).map(|u| u.name);
assert(top_users == ["Alice", "Carol"]);

// 4. Find the highest single score across all users
fn my_max(a, b) {
  if (a > b) { return a; }
  return b;
}
fn max_score(users) {
  let all_scores = [];
  for u in users {
    for s in u.scores {
      all_scores.push(s);
    }
  }
  return all_scores.reduce(0, |a, b| my_max(a, b));
}
assert(max_score(data.users) == 100);

// 5. Compute total scores per user as pairs
fn user_total(u) {
  return u.scores.reduce(0, |a, b| a + b);
}
let summaries = data.users.map(|u| [u.name, user_total(u)]);
assert(summaries[0] == ["Alice", 274]);
assert(summaries[1] == ["Bob", 253]);

println("json_process: all assertions passed");`,
  },
  {
    id: 'macros',
    sourcePath: 'examples/syntax/macros.lk',
    code: `use { vec, assert_eq, matches } from macros;

macro_rules! unless {
    ($condition:expr $body:block) => { if (!($condition)) $body };
}

let values = vec![1, 2 + 3, 4];
assert_eq!(values.1, 5);
assert(matches!(values.1, 5));

let ran = false;
unless!(values.0 == 9 {
    ran = true;
});

assert(ran);

#[derive(Debug)]
struct MacroPoint {
    value: Int,
}

let point = MacroPoint { value: values.1 };
assert_eq!("\${point}", "MacroPoint { value: 5 }");

#[cfg(false)]
fn selected_value() {
    return 0;
}

#[cfg(true)]
fn selected_value() {
    return values.1;
}

assert_eq!(selected_value(), 5);
return values.1;`,
  },
]
