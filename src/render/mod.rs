//! Output (WU-C). Every function returns text without a trailing newline; the CLI adds it.
//! Pure: no clock, no environment, no filesystem; the same input renders the same bytes.

mod brief;
mod common;
mod html;
mod table;
mod text;
mod why;

use crate::model::Document;
use serde::{Deserialize, Serialize};

pub use brief::brief;
pub use html::html;
pub use table::{header, table};
pub use text::{history, plan, verify_report};
pub use why::why;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderOpts {
    pub unicode: bool,
    pub color: bool,
    /// Terminal columns; rows are cut to it (default 120).
    pub width: Option<usize>,
}

/// Schema `harn/1`, serde pretty (2-space).
pub fn json(doc: &Document) -> String {
    // Fails only on a path that is not UTF-8; the signature has no error channel.
    serde_json::to_string_pretty(doc).expect("harn/1 document serializes (paths must be UTF-8)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SCHEMA;

    #[test]
    fn json_is_pretty_two_space_with_the_schema_first() {
        let doc = Document {
            schema: SCHEMA.to_string(),
            generated_at: "2026-10-13T00:00:00Z".to_string(),
            agents: Vec::new(),
        };
        assert_eq!(
            json(&doc),
            "{\n  \"schema\": \"harn/1\",\n  \"generated_at\": \"2026-10-13T00:00:00Z\",\n  \"agents\": []\n}"
        );
    }
}
