//! Meaning vectors: the embedding model turns text into numbers, and similar meaning gives similar numbers.

use crate::summarizer;
use anyhow::{Result, bail};
use futures::channel::oneshot;
use std::path::PathBuf;

pub struct Embedded {
    pub query: Vec<f32>,
    pub passages: Vec<Vec<f32>>,
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Embeds a question and passages on a background thread. The model expects "query:" and "passage:" prefixes.
pub fn embed(model: PathBuf, question: String, passages: Vec<String>) -> oneshot::Receiver<Result<Embedded>> {
    let (tx, rx) = oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(run(&model, &question, &passages));
    });
    rx
}

fn run(model: &PathBuf, question: &str, passages: &[String]) -> Result<Embedded> {
    let mut input = format!("query: {}\n", one_line(question));
    for p in passages {
        input.push_str(&format!("passage: {}\n", one_line(p)));
    }
    let out = summarizer::run_helper(model, "-", "embed", &input)?;
    let mut vectors: Vec<Vec<f32>> = out
        .lines()
        .map(|l| l.split_whitespace().filter_map(|x| x.parse().ok()).collect())
        .collect();
    if vectors.len() != passages.len() + 1 || vectors.iter().any(|v| v.is_empty()) {
        bail!("the embedding model returned an unexpected result");
    }
    let query = vectors.remove(0);
    Ok(Embedded { query, passages: vectors })
}

pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

pub fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub fn from_blob(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vectors_survive_the_database_encoding() {
        let v = vec![0.25, -1.5, 3.0e-3];
        assert_eq!(from_blob(&to_blob(&v)), v);
    }

    #[test]
    fn similar_directions_score_higher() {
        assert!(dot(&[1.0, 0.0], &[0.9, 0.1]) > dot(&[1.0, 0.0], &[0.1, 0.9]));
    }

    #[test]
    fn text_is_flattened_to_one_line() {
        assert_eq!(one_line("a\nb   c"), "a b c");
    }
}
