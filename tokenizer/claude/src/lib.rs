mod utok {
    pub(crate) mod claude;
    #[allow(dead_code, reason = "vendored universal tokenizer unit abstraction")]
    pub(crate) mod utf;
}

pub use utok::claude::Family;

/// Count message content with the embedded reconstruction, excluding chat framing.
pub fn count_units(units: &[u16], family: Family) -> u32 {
    utok::claude::content_token_count(units, family)
}
