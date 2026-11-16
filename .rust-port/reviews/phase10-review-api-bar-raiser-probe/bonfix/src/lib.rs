//! The fix for the map setters: bon's `with = FromIterator::from_iter` next to `default`.
use indexmap::IndexMap;

#[derive(Debug, Default, bon::Builder)]
pub struct Config {
    #[builder(default, with = FromIterator::from_iter)]
    pub trace_attributes: IndexMap<String, i64>,
    #[builder(default, with = FromIterator::from_iter)]
    pub baggage_attributes: IndexMap<String, String>,
}

pub fn pairs() -> Config {
    Config::builder()
        .trace_attributes([("run".to_owned(), 7)])
        .baggage_attributes([("tenant".to_owned(), "a".to_owned())])
        .build()
}
pub fn a_map_still_works(map: IndexMap<String, i64>) -> Config {
    Config::builder().trace_attributes(map).build()
}
