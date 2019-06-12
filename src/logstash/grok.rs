// Copyright 2018-2019, Wayfair GmbH
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::errors::*;
use grok::Grok;
use simd_json::json;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::str;
use tremor_script::LineValue;

const PATTERNS_FILE_TUPLE: &str = "%{NOTSPACE:alias} %{GREEDYDATA:pattern}";
const PATTERNS_FILE_DEFAULT_PATH: &str = "/etc/tremor/grok.patterns";

pub fn resolve(pattern: String) -> GrokPattern {
    GrokPattern::from_file(PATTERNS_FILE_DEFAULT_PATH.to_string(), pattern)
        .expect("Could not create grok pattern recognizer")
}

pub struct GrokPattern {
    pub definition: String,
    pub pattern: grok::Pattern,
}

impl GrokPattern {
    pub fn from_file(file_path: String, definition: String) -> Result<GrokPattern> {
        let file = File::open(file_path.clone())?;
        let input: Box<BufRead> = Box::new(BufReader::new(file));

        let mut grok = Grok::default();
        let recognizer = grok.compile(&PATTERNS_FILE_TUPLE, true)?;

        let mut result = Grok::default();

        let mut error_count = 0;
        for (_num, line) in input.lines().enumerate() {
            let l = line?;
            if l.is_empty() || l.starts_with('#') {
                continue;
            }

            match recognizer.match_against(&l) {
                Some(m) => {
                    if let Some(alias) = m.get("alias") {
                        let pattern = m.get("pattern").expect("Expected a non-NONE value");
                        result.insert_definition(alias.to_string(), pattern.to_string())
                    } else {
                        return Err(
                            format!("{}: {:?}", "Expected a non-NONE value", (_num, &l)).into()
                        );
                    }
                }
                None => {
                    dbg!(("error in pattern on line", _num, &l));
                    error_count += 1
                }
            }
        }

        if error_count > 0 {
            return Err("Bad patterns file".into());
        }

        Ok(GrokPattern {
            definition: format!("{}{}", "file://", file_path),
            pattern: result.compile(&definition, true)?,
        })
    }

    pub fn new(definition: String) -> Result<Self> {
        let mut grok = Grok::default();
        if let Ok(pattern) = grok.compile(&definition, true) {
            Ok(GrokPattern {
                definition,
                pattern,
            })
        } else {
            Err(format!("Failed to compile logstash grok pattern `{}`", definition).into())
        }
    }

    pub fn matches(&self, data: Vec<u8>) -> Result<LineValue> {
        let text: String = str::from_utf8(&data)?.to_string();
        match self.pattern.match_against(&text) {
            Some(m) => {
                dbg!(&m);
                let mut as_map: HashMap<&str, &str> = HashMap::new();
                for (a, b) in m.iter() {
                    as_map.insert(a, b);
                }
                let as_json = json!(as_map);
                dbg!(&as_map);
                Ok(LineValue::new(Box::new(vec![]), |_| as_json.into()))
            }
            None => Err(format!("No match for log text: {}", &text).into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! assert_grok_ok {
        ($pattern:expr, $raw:expr, $json:expr) => {
            let pat: String = $pattern.to_string();
            let raw: String = $raw.to_string();
            dbg!(pat.clone());
            dbg!(raw.clone());
            let codec = GrokPattern::new(pat).expect("bad pattern");
            let decoded = codec.matches(raw.as_bytes().to_vec());
            dbg!(&decoded);
            match decoded {
                Ok(j) => assert_eq!(j, $json),

                Err(x) => eprintln!("{}", x),
            }
        };
    }

    macro_rules! assert_grok_ko {
        ($pattern:expr, $raw:expr, $expr:expr) => {
            let pat: String = $pattern.to_string();
            let raw: String = $raw.to_string();
            dbg!(pat.clone());
            dbg!(raw.clone());
            let codec = GrokPattern::new(pat).expect("bad pattern");
            let decoded = codec.matches(raw.as_bytes().to_vec());
            match decoded {
                Err(decoded) => assert_eq!($expr, decoded.description()),
                _ => eprintln!("{}", "Expected no match"),
            };
        };
    }

    #[test]
    fn no_match() {
        assert_grok_ko!(
            "{}",
            "cookie monster",
            "No match for log text: cookie monster"
        );
    }

    #[test]
    fn decode_no_alias_does_not_map() {
        assert_grok_ok!("%{USERNAME}", "foobar", json!({}));
    }

    #[test]
    fn decode_clashing_alias_maps_with_default_unknowable_name() {
        assert_grok_ok!(
            "%{USERNAME:snot} %{USERNAME:snot}",
            "foobar badger",
            json!({"snot": "badger", "name0": "foobar"})
        );
        assert_grok_ok!(
            "%{USERNAME:snot} %{USERNAME:name0}",
            "foobar badger",
            json!({"snot": "foobar", "name0": "badger"})
        );
    }

    #[test]
    fn decode_simple_ok() {
        assert_grok_ok!(
            "%{USERNAME:username}",
            "foobar",
            json!({"username": "foobar"})
        );
    }

    #[test]
    fn decode_syslog_esx_hypervisors() {
        let pattern = r#"^<%%{POSINT:syslog_pri}> %{TIMESTAMP_ISO8601:syslog_timestamp} (?<esx_uid>[-0-9a-f]+) %{SYSLOGHOST:syslog_hostname} (?:(?<syslog_program>[\x21-\x39\x3b-\x5a\x5c\x5e-\x7e]+)(?:\[%{POSINT:syslog_pid}\])?:?)? (?:%{TIMESTAMP_ISO8601:syslog_ingest_timestamp} )?(%{WORD:wf_pod} %{WORD:wf_datacenter} )?%{GREEDYDATA:syslog_message}"#;
        assert_grok_ok!(pattern,"<%1> 2019-04-01T09:59:19+0000 deadbeaf01234 example program_name[1234] 2019-04-01T09:59:19+0010 pod dc foo bar baz", json!({
           "syslog_program": "program_name",
           "esx_uid": "deadbeaf01234",
           "wf_datacenter": "dc",
           "syslog_timestamp": "2019-04-01T09:59:19+0000",
           "wf_pod": "pod",
           "syslog_message": "foo bar baz",
           "syslog_hostname": "example",
           "syslog_pid": "1234",
           "syslog_ingest_timestamp": "2019-04-01T09:59:19+0010",
           "syslog_pri": "1"
        }));
    }

    #[test]
    fn decode_syslog_artifactory() {
        let pattern =  r#"^<%%{POSINT:syslog_pri}>(?:(?<syslog_version>\d{1,3}) )?(?:%{SYSLOGTIMESTAMP:syslog_timestamp1}|%{TIMESTAMP_ISO8601:syslog_timestamp}) %{SYSLOGHOST:syslog_hostname} (?:(?<syslog_program>[\x21-\x39\x3b-\x5a\x5c\x5e-\x7e]+)(?:\[%{POSINT:syslog_pid}\])?:?)? (?:%{TIMESTAMP_ISO8601:syslog_ingest_timestamp} )?(%{WORD:wf_pod} %{WORD:wf_datacenter} )?%{GREEDYDATA:syslog_message}"#;
        assert_grok_ok!(pattern, "<%1>123 Jul   7 10:51:24 hostname program_name[1234] 2019-04-01T09:59:19+0010 pod dc foo bar baz", json!({
           "wf_pod": "pod",
           "syslog_timestamp1": "Jul   7 10:51:24",
           "syslog_message": "foo bar baz",
           "syslog_timestamp": "",
           "syslog_pri": "1",
           "syslog_hostname": "hostname",
           "wf_datacenter": "dc",
           "syslog_program": "program_name",
           "syslog_ingest_timestamp": "2019-04-01T09:59:19+0010",
           "syslog_pid": "1234",
           "syslog_version": "123"
        }));
    }

    /* TODO Needs a test-local patterns file for CI #[test]
        fn decode_windows_nxlog() {
            let pat1 = r#"%{NXLOG_SUPPLIER_CHAIN_SYSTEM} (?:%{TIMESTAMP_ISO8601:syslog_ingest_timestamp} )?(%{WORD:wf_pod} %{WORD:wf_datacenter} )?%{GREEDYDATA:syslog_message}"#;
            let pat2 = r#"%{NXLOG_SQL_SERVER} (?:%{TIMESTAMP_ISO8601:syslog_ingest_timestamp} )?(%{WORD:wf_pod} %{WORD:wf_datacenter} )?%{GREEDYDATA:syslog_message}"#;

            assert_grok_ok!(
                pat1,
                "<blork 2019-04-01T09:59:19+0010 pod dc foo bar baz",
                json!({})
            );

            assert_grok_ok!(
                pat2,
                "<blork 2019-04-01T09:59:19+0010 pod dc foo bar baz",
                json!({})
            );
        }
    */

    #[test]
    fn decode_standard_syslog() {
        let pattern = r#"^<%%{POSINT:syslog_pri}>(?:(?<syslog_version>\d{1,3}) )?(?:%{SYSLOGTIMESTAMP:syslog_timestamp0}|%{TIMESTAMP_ISO8601:syslog_timestamp1}) %{SYSLOGHOST:syslog_hostname}  ?(?:%{TIMESTAMP_ISO8601:syslog_ingest_timestamp} )?(%{WORD:wf_pod} %{WORD:wf_datacenter} )?%{GREEDYDATA:syslog_message}"#;

        assert_grok_ok!(
            pattern,
            "<%1>123 Jul   7 10:51:24 hostname 2019-04-01T09:59:19+0010 pod dc foo bar baz",
            json!({
               "syslog_timestamp1": "",
               "syslog_ingest_timestamp": "2019-04-01T09:59:19+0010",
               "wf_datacenter": "dc",
               "syslog_hostname": "hostname",
               "syslog_pri": "1",
               "wf_pod": "pod",
               "syslog_message": "foo bar baz",
               "syslog_version": "123",
               "syslog_timestamp0": "Jul   7 10:51:24"
            })
        );
    }

    #[test]
    fn decode_nonstd_syslog() {
        let pattern = r#"^<%%{POSINT:syslog_pri}>(?:(?<syslog_version>\d{1,3}) )?(?:%{SYSLOGTIMESTAMP:syslog_timestamp}|%{TIMESTAMP_ISO8601:syslog_timestamp})  ?(?:%{TIMESTAMP_ISO8601:syslog_ingest_timestamp} )?(%{WORD:wf_pod} %{WORD:wf_datacenter} )?%{GREEDYDATA:syslog_message}"#;

        assert_grok_ok!(
            pattern,
            "<%1>123 Jul   7 10:51:24  2019-04-01T09:59:19+0010 pod dc foo bar baz",
            json!({
               "syslog_version": "123",
               "syslog_message": "foo bar baz",
               "syslog_ingest_timestamp": "2019-04-01T09:59:19+0010",
               "syslog_timestamp": "",
               "syslog_pri": "1",
               "wf_pod": "pod",
               "wf_datacenter": "dc",
               "name1": "Jul   7 10:51:24"
            })
        );
    }

    #[test]
    fn decode_syslog_noversion() {
        let pattern = r#"^<%%{POSINT:syslog_pri}>(?:(?<syslog_version>\d{1,3}))? ?(?:%{TIMESTAMP_ISO8601:syslog_ingest_timestamp} )?(%{WORD:wf_pod} %{WORD:wf_datacenter} )?%{GREEDYDATA:syslog_message}"#;

        assert_grok_ok!(
            pattern,
            "<%1>123 2019-04-01T09:59:19+0010 pod dc foo bar baz",
            json!({
               "wf_pod": "pod",
               "syslog_pri": "1",
               "wf_datacenter": "dc",
               "syslog_version": "123",
               "syslog_message": "foo bar baz",
               "syslog_ingest_timestamp": "2019-04-01T09:59:19+0010"
            })
        );
        assert_grok_ok!(
            pattern,
            "<%1>12 2019-04-01T09:59:19+0010 pod dc foo bar baz",
            json!({
               "wf_pod": "pod",
               "syslog_pri": "1",
               "wf_datacenter": "dc",
               "syslog_version": "12",
               "syslog_message": "foo bar baz",
               "syslog_ingest_timestamp": "2019-04-01T09:59:19+0010"
            })
        );
        assert_grok_ok!(
            pattern,
            "<%1>1 2019-04-01T09:59:19+0010 pod dc foo bar baz",
            json!({
               "wf_pod": "pod",
               "syslog_pri": "1",
               "wf_datacenter": "dc",
               "syslog_version": "1",
               "syslog_message": "foo bar baz",
               "syslog_ingest_timestamp": "2019-04-01T09:59:19+0010"
            })
        );
        assert_grok_ok!(
            pattern,
            "<%1> 2019-04-01T09:59:19+0010 pod dc foo bar baz",
            json!({
               "wf_pod": "pod",
               "syslog_pri": "1",
               "wf_datacenter": "dc",
               "syslog_version": "",
               "syslog_message": "foo bar baz",
               "syslog_ingest_timestamp": "2019-04-01T09:59:19+0010"
            })
        );
    }

    #[test]
    fn decode_syslog_isilon() {
        let pattern = r#"^<invld>%{GREEDYDATA:syslog_error_prefix}>(%{TIMESTAMP_ISO8601:syslog_timestamp}(?: %{TIMESTAMP_ISO8601:syslog_ingest_timestamp})? )?(%{WORD:wf_pod} %{WORD:wf_datacenter} )?%{SYSLOGHOST:syslog_hostname} ?(%{SYSLOGPROG:syslog_program}:)?%{GREEDYDATA:syslog_message}"#;
        assert_grok_ok!(
            pattern,
            "<invld>x>hostname.com bar baz",
            json!({
               "pid": "",
               "syslog_program": "",
               "syslog_timestamp": "",
               "syslog_message": "bar baz",
               "wf_datacenter": "",
               "syslog_hostname": "hostname.com",
               "syslog_error_prefix": "x",
               "wf_pod": "",
               "syslog_ingest_timestamp": "",
               "program": ""
            })
        );

        assert_grok_ok!(
            pattern,
            "<invld>x y z>2019-04-01T09:59:19+0010 2019-04-01T09:59:19+0010 hostname.com bar baz",
            json!({
               "syslog_timestamp": "2019-04-01T09:59:19+0010",
               "syslog_ingest_timestamp": "2019-04-01T09:59:19+0010",
               "wf_datacenter": "",
               "wf_pod": "",
               "syslog_hostname": "hostname.com",
               "syslog_program": "",
               "syslog_message": "bar baz",
               "pid": "",
               "syslog_error_prefix": "x y z",
               "program": ""
            })
        );

        assert_grok_ok!(
            pattern,
            "<invld>x y z>2019-04-01T09:59:19+0010 hostname.com bar baz",
            json!({
               "syslog_timestamp": "2019-04-01T09:59:19+0010",
               "syslog_ingest_timestamp": "",
               "wf_datacenter": "",
               "wf_pod": "",
               "syslog_hostname": "hostname.com",
               "syslog_program": "",
               "syslog_message": "bar baz",
               "pid": "",
               "syslog_error_prefix": "x y z",
               "program": ""
            })
        );

        assert_grok_ok!(
            pattern,
            "<invld>x y z>2019-04-01T09:59:19+0010 pod dc hostname.com bar baz",
            json!({
               "syslog_timestamp": "2019-04-01T09:59:19+0010",
               "syslog_ingest_timestamp": "",
               "wf_datacenter": "dc",
               "wf_pod": "pod",
               "syslog_hostname": "hostname.com",
               "syslog_program": "",
               "syslog_message": "bar baz",
               "pid": "",
               "syslog_error_prefix": "x y z",
               "program": ""
            })
        );

        assert_grok_ok!(
            pattern,
            "<invld>x y z>pod dc hostname.com bar baz",
            json!({
               "syslog_timestamp": "",
               "syslog_ingest_timestamp": "",
               "wf_datacenter": "dc",
               "wf_pod": "pod",
               "syslog_hostname": "hostname.com",
               "syslog_program": "",
               "syslog_message": "bar baz",
               "pid": "",
               "syslog_error_prefix": "x y z",
               "program": ""
            })
        );

        // prog / pid
        assert_grok_ok!(
            pattern,
            "<invld>x>hostname.com program_name[1234]: bar baz",
            json!({
               "pid": "1234",
               "syslog_program": "program_name[1234]",
               "syslog_timestamp": "",
               "syslog_message": " bar baz",
               "wf_datacenter": "",
               "syslog_hostname": "hostname.com",
               "syslog_error_prefix": "x",
               "wf_pod": "",
               "syslog_ingest_timestamp": "",
               "program": "program_name"
            })
        );

        assert_grok_ok!(pattern, "<invld>x y z>2019-04-01T09:59:19+0010 2019-04-01T09:59:19+0010 hostname.com program_name: bar baz", json!({
           "syslog_timestamp": "2019-04-01T09:59:19+0010",
           "syslog_ingest_timestamp": "2019-04-01T09:59:19+0010",
           "wf_datacenter": "",
           "wf_pod": "",
           "syslog_hostname": "hostname.com",
           "syslog_program": "program_name",
           "syslog_message": " bar baz",
           "pid": "",
           "syslog_error_prefix": "x y z",
           "program": "program_name"
        }));

        assert_grok_ok!(
            pattern,
            "<invld>x y z>2019-04-01T09:59:19+0010 hostname.com program_name[1234]: bar baz",
            json!({
               "syslog_timestamp": "2019-04-01T09:59:19+0010",
               "syslog_ingest_timestamp": "",
               "wf_datacenter": "",
               "wf_pod": "",
               "syslog_hostname": "hostname.com",
               "syslog_program": "program_name[1234]",
               "syslog_message": " bar baz",
               "pid": "1234",
               "syslog_error_prefix": "x y z",
               "program": "program_name"
            })
        );

        assert_grok_ok!(
            pattern,
            "<invld>x y z>2019-04-01T09:59:19+0010 pod dc hostname.com program_name: bar baz",
            json!({
               "syslog_timestamp": "2019-04-01T09:59:19+0010",
               "syslog_ingest_timestamp": "",
               "wf_datacenter": "dc",
               "wf_pod": "pod",
               "syslog_hostname": "hostname.com",
               "syslog_program": "program_name",
               "syslog_message": " bar baz",
               "pid": "",
               "syslog_error_prefix": "x y z",
               "program": "program_name"
            })
        );

        assert_grok_ok!(
            pattern,
            "<invld>x y z>pod dc hostname.com program_name[1234]:bar baz",
            json!({
               "syslog_timestamp": "",
               "syslog_ingest_timestamp": "",
               "wf_datacenter": "dc",
               "wf_pod": "pod",
               "syslog_hostname": "hostname.com",
               "syslog_program": "program_name[1234]",
               "syslog_message": "bar baz",
               "pid": "1234",
               "syslog_error_prefix": "x y z",
               "program": "program_name"
            })
        );
    }
}