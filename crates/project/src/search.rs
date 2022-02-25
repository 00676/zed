use aho_corasick::{AhoCorasick, AhoCorasickBuilder};
use anyhow::Result;
use language::{char_kind, Rope};
use regex::{Regex, RegexBuilder};
use smol::future::yield_now;
use std::{
    io::{BufRead, BufReader, Read},
    ops::Range,
    sync::Arc,
};

#[derive(Clone)]
pub enum SearchQuery {
    Text {
        search: Arc<AhoCorasick<usize>>,
        query: String,
        whole_word: bool,
    },
    Regex {
        multiline: bool,
        regex: Regex,
    },
}

impl SearchQuery {
    pub fn text(query: impl ToString, whole_word: bool, case_sensitive: bool) -> Self {
        let query = query.to_string();
        let search = AhoCorasickBuilder::new()
            .auto_configure(&[&query])
            .ascii_case_insensitive(!case_sensitive)
            .build(&[&query]);
        Self::Text {
            search: Arc::new(search),
            query,
            whole_word,
        }
    }

    pub fn regex(query: impl ToString, whole_word: bool, case_sensitive: bool) -> Result<Self> {
        let mut query = query.to_string();
        if whole_word {
            let mut word_query = String::new();
            word_query.push_str("\\b");
            word_query.push_str(&query);
            word_query.push_str("\\b");
            query = word_query
        }

        let multiline = query.contains("\n") || query.contains("\\n");
        let regex = RegexBuilder::new(&query)
            .case_insensitive(!case_sensitive)
            .multi_line(multiline)
            .build()?;
        Ok(Self::Regex { multiline, regex })
    }

    pub fn detect<T: Read>(&self, stream: T) -> Result<bool> {
        match self {
            SearchQuery::Text { search, .. } => {
                let mat = search.stream_find_iter(stream).next();
                match mat {
                    Some(Ok(_)) => Ok(true),
                    Some(Err(err)) => Err(err.into()),
                    None => Ok(false),
                }
            }
            SearchQuery::Regex { multiline, regex } => {
                let mut reader = BufReader::new(stream);
                if *multiline {
                    let mut text = String::new();
                    if let Err(err) = reader.read_to_string(&mut text) {
                        Err(err.into())
                    } else {
                        Ok(regex.find(&text).is_some())
                    }
                } else {
                    for line in reader.lines() {
                        let line = line?;
                        if regex.find(&line).is_some() {
                            return Ok(true);
                        }
                    }
                    Ok(false)
                }
            }
        }
    }

    pub async fn search(&self, rope: &Rope) -> Vec<Range<usize>> {
        const YIELD_INTERVAL: usize = 20000;

        let mut matches = Vec::new();
        match self {
            SearchQuery::Text {
                search, whole_word, ..
            } => {
                for (ix, mat) in search
                    .stream_find_iter(rope.bytes_in_range(0..rope.len()))
                    .enumerate()
                {
                    if (ix + 1) % YIELD_INTERVAL == 0 {
                        yield_now().await;
                    }

                    let mat = mat.unwrap();
                    if *whole_word {
                        let prev_kind = rope.reversed_chars_at(mat.start()).next().map(char_kind);
                        let start_kind = char_kind(rope.chars_at(mat.start()).next().unwrap());
                        let end_kind = char_kind(rope.reversed_chars_at(mat.end()).next().unwrap());
                        let next_kind = rope.chars_at(mat.end()).next().map(char_kind);
                        if Some(start_kind) == prev_kind || Some(end_kind) == next_kind {
                            continue;
                        }
                    }
                    matches.push(mat.start()..mat.end())
                }
            }
            SearchQuery::Regex { multiline, regex } => {
                if *multiline {
                    let text = rope.to_string();
                    for (ix, mat) in regex.find_iter(&text).enumerate() {
                        if (ix + 1) % YIELD_INTERVAL == 0 {
                            yield_now().await;
                        }

                        matches.push(mat.start()..mat.end());
                    }
                } else {
                    let mut line = String::new();
                    let mut line_offset = 0;
                    for (chunk_ix, chunk) in rope.chunks().chain(["\n"]).enumerate() {
                        if (chunk_ix + 1) % YIELD_INTERVAL == 0 {
                            yield_now().await;
                        }

                        for (newline_ix, text) in chunk.split('\n').enumerate() {
                            if newline_ix > 0 {
                                for mat in regex.find_iter(&line) {
                                    let start = line_offset + mat.start();
                                    let end = line_offset + mat.end();
                                    matches.push(start..end);
                                }

                                line_offset += line.len() + 1;
                                line.clear();
                            }
                            line.push_str(text);
                        }
                    }
                }
            }
        }
        matches
    }
}
