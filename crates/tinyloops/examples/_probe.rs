fn main() {
    let scope = serde_json::json!({
        "nodes": { "attempt": { "item": {"json": {"report": "ok"}}, "items": [{"json": {"report":"ok"}}] } }
    });
    for expr in ["=.nodes.attempt.output", "=nodes.attempt.item.json", "=.nodes.attempt.item.json"] {
        let v = tinyflows::expr::evaluate(&serde_json::Value::String(expr.to_string()), &scope);
        println!("{expr:>32}  ->  {v}");
    }
}
