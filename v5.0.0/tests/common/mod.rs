// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Shared test helpers: a scratch folder, a recording fake HTTP client and a fake
//! environment, so no test ever touches the network or the real environment.

#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use osintropy::http::{Http, Pairs, SourceError};
use osintropy::sources::{Context, Env, PersonQuery};
use serde_json::{Map, Value};

/// A folder removed when the test ends.
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new() -> Scratch {
        let path =
            std::env::temp_dir().join(format!("osintropy-test-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&path).expect("make scratch folder");
        Scratch(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One recorded request: URL, parameters and headers.
#[derive(Clone, Debug)]
pub struct Call {
    pub url: String,
    pub params: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
}

impl Call {
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// Answers each request with the next queued JSON value and records what was asked.
pub struct FakeHttp {
    responses: VecDeque<Value>,
    pub calls: Vec<Call>,
}

impl FakeHttp {
    pub fn new(responses: Vec<Value>) -> FakeHttp {
        FakeHttp {
            responses: responses.into(),
            calls: Vec::new(),
        }
    }

    fn take(
        &mut self,
        url: &str,
        params: &Pairs<'_>,
        headers: &Pairs<'_>,
    ) -> Result<Value, SourceError> {
        let own = |pairs: &Pairs<'_>| {
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect()
        };
        self.calls.push(Call {
            url: url.to_owned(),
            params: own(params),
            headers: own(headers),
        });
        self.responses
            .pop_front()
            .ok_or_else(|| SourceError::new("no queued response"))
    }
}

impl Http for FakeHttp {
    fn get_json(
        &mut self,
        url: &str,
        params: &Pairs<'_>,
        headers: &Pairs<'_>,
    ) -> Result<Map<String, Value>, SourceError> {
        match self.take(url, params, headers)? {
            Value::Object(map) => Ok(map),
            _ => Err(SourceError::new("expected a JSON object")),
        }
    }

    fn get_json_array(
        &mut self,
        url: &str,
        params: &Pairs<'_>,
        headers: &Pairs<'_>,
    ) -> Result<Vec<Value>, SourceError> {
        match self.take(url, params, headers)? {
            Value::Array(items) => Ok(items),
            _ => Err(SourceError::new("expected a JSON array")),
        }
    }

    fn post_form_json(
        &mut self,
        url: &str,
        fields: &Pairs<'_>,
    ) -> Result<Map<String, Value>, SourceError> {
        match self.take(url, fields, &[])? {
            Value::Object(map) => Ok(map),
            _ => Err(SourceError::new("expected a JSON object")),
        }
    }
}

/// Credentials from a fixed list instead of the process environment.
pub struct FakeEnv(pub HashMap<String, String>);

impl FakeEnv {
    pub fn with(pairs: &[(&str, &str)]) -> FakeEnv {
        FakeEnv(
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
        )
    }
}

impl Env for FakeEnv {
    fn var(&self, name: &str) -> Option<String> {
        self.0.get(name).filter(|value| !value.is_empty()).cloned()
    }
}

/// Run a person connector against queued responses.
pub fn person<F>(
    connector: F,
    first: &str,
    last: &str,
    location: &str,
    env: &[(&str, &str)],
    responses: Vec<Value>,
) -> (
    Result<Vec<osintropy::models::Observation>, SourceError>,
    Vec<Call>,
)
where
    F: Fn(
        &PersonQuery,
        &mut Context<'_>,
    ) -> Result<Vec<osintropy::models::Observation>, SourceError>,
{
    let mut http = FakeHttp::new(responses);
    let env = FakeEnv::with(env);
    let query = PersonQuery {
        first: first.into(),
        last: last.into(),
        location: location.into(),
    };
    let result = {
        let mut ctx = Context {
            http: &mut http,
            env: &env,
        };
        connector(&query, &mut ctx)
    };
    (result, http.calls)
}

/// Run an organization or domain connector against queued responses.
pub fn named<F>(
    connector: F,
    name: &str,
    env: &[(&str, &str)],
    responses: Vec<Value>,
) -> (
    Result<Vec<osintropy::models::Observation>, SourceError>,
    Vec<Call>,
)
where
    F: Fn(&str, &mut Context<'_>) -> Result<Vec<osintropy::models::Observation>, SourceError>,
{
    let mut http = FakeHttp::new(responses);
    let env = FakeEnv::with(env);
    let result = {
        let mut ctx = Context {
            http: &mut http,
            env: &env,
        };
        connector(name, &mut ctx)
    };
    (result, http.calls)
}
