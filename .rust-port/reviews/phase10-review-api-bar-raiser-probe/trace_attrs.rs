//! TS: `new Agent({ traceAttributes: { 'session.id': 'abc', 'run': 7 }, baggageAttributes: { tenant: 'a' } })`.
use strands::telemetry::AttributeValue;

// the map type of the two fields through strands
pub fn a() -> strands::indexmap::IndexMap<String, AttributeValue> { todo!() }
pub fn b() -> strands::telemetry::indexmap::IndexMap<String, String> { todo!() }
// an array of pairs, as the TS literal reads
pub fn c() {
    let _ = strands::Agent::builder()
        .trace_attributes([("session.id".to_owned(), AttributeValue::from("abc"))])
        .baggage_attributes([("tenant".to_owned(), "a".to_owned())]);
}
// control: what compiles without an `indexmap` dependency
pub fn ok() {
    let _ = strands::Agent::builder()
        .trace_attributes([("session.id".to_owned(), "abc".into()), ("run".to_owned(), 7.into())].into_iter().collect())
        .baggage_attributes([("tenant".to_owned(), "a".to_owned())].into_iter().collect());
}
