export type PlaygroundExample = {
  name: string
  code: string
  expectFailure?: boolean
}

export const playgroundExamples: PlaygroundExample[] = [
  {
    name: 'Pattern match',
    code: `let status = 404;
let label = match status {
  200 => "OK",
  301 | 302 => "Redirect",
  404 => "Not Found",
  _ => "Unknown",
};

println("status {}: {}", status, label);
return label;`,
  },
  {
    name: 'Destructuring',
    code: `let user = { "name": "Mira", "roles": ["admin", "editor"], "active": true };
let { "name": name, "roles": [primary, ..rest] } = user;

if let [next, .._] = rest {
  println("{} handles {} then {}", name, primary, next);
}

return "\${name}:\${primary}:\${rest.len()}";`,
  },
  {
    name: 'Struct traits',
    code: `struct Rect { w: Int, h: Int }

trait Area {
  fn area(self) -> Int;
}

trait Describe {
  fn describe(self) -> String;
}

impl Area for Rect {
  fn area(self) -> Int { return self.w * self.h; }
}

impl Describe for Rect {
  fn describe(self) -> String { return "Rect(\${self.w}x\${self.h})"; }
}

let shape = Rect { w: 8, h: 5 };
println("{} area={}", shape.describe(), shape.area());
return shape.area();`,
  },
  {
    name: 'Named params',
    code: `fn range_sum(start, stop, { step: Int? = 1, label: String? = "sum" }) {
  let acc = 0;
  let i = start;
  let step_val = step ?? 1;
  let label_text = label ?? "sum";

  while (i <= stop) {
    acc += i;
    i += step_val;
  }

  println("{}: {}", label_text, acc);
  return acc;
}

return range_sum(0, 8, label: "evens", step: 2);`,
  },
  {
    name: 'Collections',
    code: `let scores = [9, 12, 7, 12];
scores.push(15);

let profile = { "name": "Iris", "scores": scores };
let best = profile.scores.sort().reverse()[0];
let unique = profile.scores.unique();

println("{} best={} unique={}", profile.name, best, unique.len());
return best;`,
  },
  {
    name: 'Ranges',
    code: `use iter;

let inclusive = 1..=5;
let stepped = iter.range(0, 10, 2);
let total = 0;

for value in inclusive {
  total += value;
}

println("range total={}, stepped={}", total, stepped);
return total + stepped.len();`,
  },
  {
    name: 'Iter pipeline',
    code: `use iter;

let values = iter.range(1, 8);
let doubled = iter.map(values, |value| value * 2);
let large = iter.filter(doubled, |value| value > 8);
let total = iter.reduce(large, 0, |acc, value| acc + value);

println("total = {}", total);
return total;`,
  },
  {
    name: 'Browser guard',
    code: `use fs;
println("this line will not run");`,
    expectFailure: true,
  },
]
