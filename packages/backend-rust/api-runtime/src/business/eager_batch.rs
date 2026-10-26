/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
//! Shared eager packing with the existing preparation/async rejection order.
use std::{
    future::{Future, poll_fn},
    pin::Pin,
    task::Poll,
};

// JavaScript Promise.all receives an eagerly-created promise for EVERY post,
// even when an earlier post synchronously fails its Date conversion. Poll all
// newly-created futures once before exposing a ready error; retain input order.
pub(super) enum PackFailure<E> {
    Preparation(E),
    Async(E),
}
pub(super) async fn eager_batch<F, T, E>(futures: impl IntoIterator<Item = F>) -> Result<Vec<T>, E>
where
    F: Future<Output = Result<T, PackFailure<E>>>,
{
    let mut pending: Vec<_> = futures
        .into_iter()
        .map(|future| Some(Box::pin(future)))
        .collect();
    let mut output: Vec<Option<T>> = (0..pending.len()).map(|_| None).collect();
    poll_fn(move |context| {
        let mut preparation_error = None;
        let mut async_error = None;
        for (index, slot) in pending.iter_mut().enumerate() {
            let Some(future) = slot else { continue };
            match Future::poll(Pin::as_mut(future), context) {
                Poll::Pending => {}
                Poll::Ready(result) => {
                    *slot = None;
                    match result {
                        Ok(value) => output[index] = Some(value),
                        Err(PackFailure::Preparation(error)) if preparation_error.is_none() => {
                            preparation_error = Some(error)
                        }
                        Err(PackFailure::Async(error)) if async_error.is_none() => {
                            async_error = Some(error)
                        }
                        Err(_) => {}
                    }
                }
            }
        }
        if let Some(error) = preparation_error.or(async_error) {
            return Poll::Ready(Err(error));
        }
        if pending.iter().all(Option::is_none) {
            Poll::Ready(Ok(output.iter_mut().filter_map(Option::take).collect()))
        } else {
            Poll::Pending
        }
    })
    .await
}
