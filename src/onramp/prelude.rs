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

pub use super::{Onramp, OnrampAddr, OnrampImpl, OnrampMsg};
pub use crate::codec::{self, Codec};
pub use crate::errors::*;
pub use crate::preprocessor::{self, Preprocessor, Preprocessors};
pub use crate::system::{PipelineAddr, PipelineMsg};
pub use crate::url::TremorURL;
use crate::utils::nanotime;
pub use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError};
use std::mem;
pub use std::thread;
pub use tremor_pipeline::Event;

pub fn make_preprocessors(preprocessors: &[String]) -> Result<Preprocessors> {
    preprocessors
        .iter()
        .map(|n| preprocessor::lookup(&n))
        .collect()
}
// We are borrowing a dyn box as we don't want to pass ownership.
#[allow(clippy::borrowed_box)]
pub fn send_event(
    pipelines: &[(TremorURL, PipelineAddr)],
    preprocessors: &mut Preprocessors,
    codec: &mut Box<dyn Codec>,
    id: u64,
    data: Vec<u8>,
) {
    let ingest_ns = nanotime();
    let mut data = vec![data];
    let mut data1 = Vec::new();
    for pp in preprocessors {
        data1.clear();
        for (i, d) in data.iter().enumerate() {
            match pp.process(ingest_ns, d) {
                Ok(mut r) => data1.append(&mut r),
                Err(e) => {
                    error!("Preprocessor[{}] error {}", i, e);
                    return;
                }
            }
        }
        mem::swap(&mut data, &mut data1);
    }

    for d in data {
        match codec.decode(d, ingest_ns) {
            Ok(Some(value)) => {
                let event = tremor_pipeline::Event {
                    is_batch: false,
                    id,
                    meta: tremor_pipeline::MetaMap::new(),
                    value,
                    ingest_ns,
                    kind: None,
                };
                let len = pipelines.len();
                for (input, addr) in &pipelines[..len - 1] {
                    if let Some(input) = input.instance_port() {
                        let _ = addr.addr.send(PipelineMsg::Event {
                            input,
                            event: event.clone(),
                        });
                    }
                }
                let (input, addr) = &pipelines[len - 1];
                if let Some(input) = input.instance_port() {
                    let _ = addr.addr.send(PipelineMsg::Event { input, event });
                }
            }
            Ok(None) => (),
            Err(e) => error!("{}", e),
        }
    }
}
