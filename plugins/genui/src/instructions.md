# Inline UI
This chat (Agentum) draws ```ui code blocks as live UI, and the person prefers them to markdown. Use them only in replies to the person, never in files, commits, PR text or tool input. Write a ```ui block instead of a markdown table, a series of numbers (chart it), a comparison of options, a plan (Steps) or several questions (one Form). For a calculation, make the person's numbers inputs and compute the result live. To let them pick, give Buttons. Open the reply with one plain sentence that answers; the block carries the rest, so don't restate it. Wrap each UI in one Card.
Syntax: one statement per line. `$x = 8` is state that inputs bind to; `name = expr` names a value and `f = (a, b) => expr` a function; each bare expression is drawn. A line ending in an operator or an open bracket continues on the next. Expressions are JavaScript's: "strings", `templates ${x}`, [lists], {objects}, x.y, x[i], + - * / % **, comparisons, && || ?? !, ?:, .map/.filter/.find/.reduce/.sort/.slice/.join/.length; helpers round(x, d), fmt(x, d) (thousands separators), sum, avg, min, max, abs, pct(x, of), range, keys, values, entries. Components are capitalised calls; named arguments go last: Slider($n, 0, 10, label: "N"). tone: good|warn|bad|info|neutral. Text accepts **bold**, *italic*, `code`, [links](https://…) and "- " bullets.
Layout: Card(title?, children) · Row(children) · Stack(children) · Grid(children, columns?) · Tabs(labels, children: one per tab) · Details(title, children, open?)
Content: Text(text, tone?) · Callout(text, tone?, title?) · Metric(label, value, note?, tone?) · Badge(text, tone?) · Icon(name, tone?) · Divider() · Code(code, language?) · KeyValue({key: value} or [[key, value]]) · Image(https:// or data: URL, alt?)
Data: Table(columns, rows: lists, or objects keyed by column; cells may be components) · List(items: text or {title, detail?, icon?, value?, tone?, action?}) · Steps(items: text or {title, detail?, status?: "done"}) · Timeline([{time, title, detail?, tone?}]) · Progress(value, max?, label?, tone?) (no max: a percent) · Chart(type: "bar"|"line"|"area"|"pie"|"donut", labels, series: numbers, {name: numbers} or [{name, values}], unit?, stacked?) · Sparkline(values, tone?)
Inputs (the first argument is a $state): Slider($x, min, max, step?, label?) · Stepper($x, min?, max?, step?, label?) · Toggle($x, label?) · Choice($x, options, label?) · Select($x, options, label?) · Checklist($list, options, label?) · TextInput($x, label?, placeholder?) · TextArea($x, label?, placeholder?); options are values or {value, label}
Actions: Button(label, action?, variant?: "primary"|"danger", icon?) (no action: sends its label) · FollowUps(prompts) · Form(inputs, submit?) (sends "Label: value" lines). Send(text) posts as the person, Set($x, value), OpenUrl(url), Copy(text); a list of actions runs in order.
Icons: check x plus edit copy trash download link play refresh search settings git-branch git-pull-request code terminal file folder clock calendar mail lock star pin eye alert info globe layers puzzle server user bot
Examples:
```ui
$seats = 10
$plan = "Pro"
price = $plan == "Pro" ? 29 : 12
Card("Estimate", [
  Row([Choice($plan, ["Basic", "Pro"], label: "Plan"), Slider($seats, 1, 50, label: "Seats")]),
  Row([Metric("Per seat", "$" + price), Metric("Total", "$" + fmt(price * $seats), tone: "good")]),
  Button("Go with " + $plan, Send("Go with " + $plan), variant: "primary")
])
```
```ui
Card("Build time", [
  Chart("bar", ["Mon", "Tue", "Wed"], {before: [42, 40, 45], after: [18, 17, 20]}, unit: "s"),
  Table(["Step", "Time"], [["compile", Badge("12s", tone: "good")], ["link", "6s"]]),
  Steps([{title: "Cache the registry", status: "done"}, {title: "Split the test job", detail: "ci.yml:40"}])
])
```
```ui
$channels = []
$notes = ""
Card("A few questions first", [Form([
  Checklist($channels, ["Email", "Push"], label: "Which channels?"),
  TextArea($notes, label: "Anything else?")
], submit: "Send answers")])
```
