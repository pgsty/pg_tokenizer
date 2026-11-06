// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use std::sync::Arc;

use pgrx::pg_sys::errcodes::PgSqlErrorCode;
use serde_json::Value;
use tokenizers::Tokenizer;

use super::{validate_new_model_name, ModelConfig, TokenizerModel, MODEL_OBJECT_POOL};
use crate::utils::{invalid_config, raise_error};

#[derive(Debug)]
pub struct HuggingFaceModel {
    tokenizer: Tokenizer,
}

impl HuggingFaceModel {
    pub fn new(name: &str, config: &HuggingFaceConfig) -> Self {
        let tokenizer = Tokenizer::from_bytes(config.as_bytes())
            .unwrap_or_else(|e| invalid_config("Hugging Face model", name, e));
        HuggingFaceModel { tokenizer }
    }
}

// SQL token IDs are int4, but Hugging Face vocabularies accept any u32 ID.
// Check every ID the tokenizer can emit: the model vocabulary, added tokens,
// and special tokens of BERT, RoBERTa and template post-processors (also
// inside a sequence), which need not appear in the vocabulary.
fn oversized_token_id(tokenizer: &Tokenizer) -> Option<(String, u64)> {
    let limit = i32::MAX as u64;
    let mut ids: Vec<(String, u64)> = tokenizer
        .get_vocab(true)
        .into_iter()
        .map(|(token, id)| (token, id.into()))
        .collect();
    ids.extend(
        tokenizer
            .get_added_tokens_decoder()
            .into_iter()
            .map(|(id, token)| (token.content, id.into())),
    );
    if let Some(post_processor) = tokenizer.get_post_processor() {
        let value = serde_json::to_value(post_processor).expect("post-processor serializes");
        post_processor_ids(&value, &mut ids);
    }
    ids.into_iter()
        .filter(|(_, id)| *id > limit)
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
}

fn post_processor_ids(value: &Value, ids: &mut Vec<(String, u64)>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                match (key.as_str(), value) {
                    // BertProcessing and RobertaProcessing: ["[SEP]", 102]
                    ("sep" | "cls", Value::Array(pair)) => {
                        if let [Value::String(token), Value::Number(id)] = pair.as_slice() {
                            ids.extend(id.as_u64().map(|id| (token.clone(), id)));
                        }
                    }
                    // TemplateProcessing special tokens: {"id": .., "ids": [..]}
                    ("ids", Value::Array(list)) => {
                        let token = map.get("id").and_then(Value::as_str).unwrap_or_default();
                        ids.extend(
                            list.iter()
                                .filter_map(Value::as_u64)
                                .map(|id| (token.to_string(), id)),
                        );
                    }
                    _ => post_processor_ids(value, ids),
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                post_processor_ids(item, ids);
            }
        }
        _ => {}
    }
}

impl TokenizerModel for HuggingFaceModel {
    fn apply(&self, text: String) -> Vec<u32> {
        self.tokenizer.apply(text)
    }

    fn apply_batch(&self, tokens: Vec<String>) -> Vec<u32> {
        self.tokenizer.apply_batch(tokens)
    }
}

pub type HuggingFaceConfig = String;

#[pgrx::pg_extern(volatile, parallel_safe)]
fn create_huggingface_model(name: &str, config: &str) {
    validate_new_model_name(name).unwrap();

    let insert_model = r#"
        INSERT INTO tokenizer_catalog.model (name, config) VALUES ($1, $2)
        ON CONFLICT (name) DO NOTHING RETURNING 1
    "#;

    let config = config.to_string();
    let model = HuggingFaceModel::new(name, &config);
    if let Some((token, id)) = oversized_token_id(&model.tokenizer) {
        raise_error(
            PgSqlErrorCode::ERRCODE_INVALID_PARAMETER_VALUE,
            format!(
                "token ID {id} of Hugging Face model \"{name}\" is out of range for type integer"
            ),
            Some(format!(
                "Token {token:?} has ID {id}; token IDs must not exceed 2147483647."
            )),
            None,
        );
    }
    let config_str = serde_json::to_string(&ModelConfig::HuggingFace(config)).unwrap();

    pgrx::Spi::connect_mut(|client| {
        let tuptable = client
            .update(insert_model, Some(1), &[name.into(), config_str.into()])
            .unwrap();

        if tuptable.is_empty() {
            panic!("Model already exists: {}", name);
        }

        crate::cache::mark_catalog_changed();
        if MODEL_OBJECT_POOL
            .insert(name.to_string(), Arc::new(model))
            .is_some()
        {
            panic!("Model already exists: {}", name);
        }
    });
}

#[pgrx::pg_extern(volatile, parallel_safe)]
fn drop_huggingface_model(name: &str) {
    validate_new_model_name(name).unwrap();

    let delete_model = r#"
        DELETE FROM tokenizer_catalog.model WHERE name = $1 RETURNING 1
    "#;

    pgrx::Spi::connect_mut(|client| {
        let tuptable = client
            .update(delete_model, Some(1), &[name.into()])
            .unwrap();

        if tuptable.is_empty() {
            pgrx::warning!("Model not found: {}", name);
        }
    });

    crate::cache::mark_catalog_changed();
    MODEL_OBJECT_POOL.remove(name);
}

#[cfg(test)]
mod tests {
    use super::{oversized_token_id, Tokenizer};
    use serde_json::{json, Value};

    fn tokenizer(vocab: Value, post_processor: Value, added: Value) -> Tokenizer {
        let config = json!({
            "model": {"type": "WordLevel", "vocab": vocab, "unk_token": "[UNK]"},
            "post_processor": post_processor,
            "added_tokens": added,
        });
        Tokenizer::from_bytes(config.to_string()).unwrap()
    }

    fn oversized(vocab: Value, post_processor: Value, added: Value) -> Option<(String, u64)> {
        oversized_token_id(&tokenizer(vocab, post_processor, added))
    }

    const MAX: u64 = i32::MAX as u64;

    #[test]
    fn vocabulary_ids() {
        let vocab = json!({"[UNK]": 0, "max": MAX});
        assert_eq!(oversized(vocab, Value::Null, json!([])), None);
        let vocab = json!({"[UNK]": 0, "big": MAX + 1, "bigger": u32::MAX});
        assert_eq!(
            oversized(vocab, Value::Null, json!([])),
            Some(("bigger".into(), u32::MAX.into()))
        );
    }

    #[test]
    fn added_token_ids() {
        // Added tokens take their vocabulary ID when the content exists.
        let added = json!([{
            "id": 1, "content": "big", "single_word": false, "lstrip": false,
            "rstrip": false, "normalized": false, "special": true
        }]);
        let tokenizer = tokenizer(json!({"[UNK]": 0, "big": MAX + 1}), Value::Null, added);
        assert!(tokenizer
            .get_added_tokens_decoder()
            .contains_key(&(MAX as u32 + 1)));
        assert_eq!(
            oversized_token_id(&tokenizer),
            Some(("big".into(), MAX + 1))
        );
    }

    #[test]
    fn post_processor_ids() {
        let vocab = json!({"[UNK]": 0});
        let bert = |sep: u64| json!({"type": "BertProcessing", "sep": ["[SEP]", sep], "cls": ["[CLS]", 1]});
        assert_eq!(oversized(vocab.clone(), bert(MAX), json!([])), None);
        assert_eq!(
            oversized(vocab.clone(), bert(MAX + 1), json!([])),
            Some(("[SEP]".into(), MAX + 1))
        );
        let roberta = json!({
            "type": "RobertaProcessing", "sep": ["</s>", 2], "cls": ["<s>", MAX + 1],
            "trim_offsets": true, "add_prefix_space": true
        });
        assert_eq!(
            oversized(vocab.clone(), roberta, json!([])),
            Some(("<s>".into(), MAX + 1))
        );
        let template = |id: u64| {
            json!({
                "type": "TemplateProcessing",
                "single": [{"SpecialToken": {"id": "[CLS]", "type_id": 0}}, {"Sequence": {"id": "A", "type_id": 0}}],
                "pair": [{"Sequence": {"id": "A", "type_id": 0}}, {"Sequence": {"id": "B", "type_id": 1}}],
                "special_tokens": {"[CLS]": {"id": "[CLS]", "ids": [5, id], "tokens": ["[CLS]", "[X]"]}}
            })
        };
        assert_eq!(oversized(vocab.clone(), template(MAX), json!([])), None);
        assert_eq!(
            oversized(vocab.clone(), template(MAX + 1), json!([])),
            Some(("[CLS]".into(), MAX + 1))
        );
        let sequence = json!({
            "type": "Sequence",
            "processors": [{"type": "ByteLevel", "add_prefix_space": true, "trim_offsets": true, "use_regex": true}, bert(MAX + 2)]
        });
        assert_eq!(
            oversized(vocab, sequence, json!([])),
            Some(("[SEP]".into(), MAX + 2))
        );
    }
}
