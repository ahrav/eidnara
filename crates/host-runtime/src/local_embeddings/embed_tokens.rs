//! The exact untruncated model token count of one text, taken from the bundle's own tokenizer.
//!
//! Inference truncates every text at the bundle's window and pads batches, so its encodings cannot say how long a text really is.
//! This module clones that tokenizer, with its vocabulary, post-processor, and added special tokens, and disables truncation and padding on the clone, so a count names the full sequence the model would see if nothing were cut.
//! There is no second tokenizer authority: the clone is the inference tokenizer.

use tokenizers::Tokenizer;

use super::inference::InferenceError;

/// A count of model tokens, distinct from provider accounting units such as Claude token estimates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EmbedTokens(u32);

impl EmbedTokens {
    pub const fn new(count: u32) -> Self {
        Self(count)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

/// The bundle's tokenizer with truncation and padding disabled.
pub struct UntruncatedTokenizer {
    tokenizer: Tokenizer,
}

impl UntruncatedTokenizer {
    /// Takes the inference tokenizer, or a clone of it, and disables truncation and padding; everything else about it stays as inference configured it.
    pub fn from_tokenizer(mut tokenizer: Tokenizer) -> Self {
        // Clearing truncation validates nothing, so it cannot fail.
        let _ = tokenizer.with_truncation(None);
        tokenizer.with_padding(None);
        debug_assert!(tokenizer.get_truncation().is_none() && tokenizer.get_padding().is_none());
        Self { tokenizer }
    }

    /// The full sequence length of `text`, including the special tokens the post-processor adds, without copying the sequence out.
    ///
    /// # Errors
    ///
    /// Returns [`InferenceError::Input`] when the tokenizer refuses the text and [`InferenceError::Invariant`] when the sequence is longer than a `u32` count can name.
    pub fn count(&self, text: &str) -> Result<EmbedTokens, InferenceError> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| InferenceError::Input(format!("tokenization failed: {e}")))?;
        u32::try_from(encoding.get_ids().len())
            .map(EmbedTokens)
            .map_err(|_| {
                InferenceError::Invariant(
                    "token sequence length exceeds the count range".to_owned(),
                )
            })
    }
}
