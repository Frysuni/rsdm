//! Runtime ownership at command boundaries; the Greeter remains synchronous.

use std::future::Future;

use anyhow::Result;

pub(crate) fn run<T>(operation: impl Future<Output = Result<T>>) -> Result<T> {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(operation)
}
