/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

/*! Defines the core async wait for one waker. */

use core::{
    mem::MaybeUninit,
    ops::DerefMut,
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    task::{Context, Poll, Waker},
};

use spin::mutex::SpinMutex;

#[cfg(feature = "alloc")]
extern crate alloc;

pub type FenceWaker = SpinMutex<MaybeUninit<Waker>>;

#[derive(Debug)]
pub struct Fence<Queue: AsRef<[FenceWaker]>> {
    queue: Queue,
    queue_pos: AtomicUsize,
    finished: AtomicBool,
}

impl<Queue> Fence<Queue>
where
    Queue: AsRef<[FenceWaker]>,
{
    pub const fn new(queue: Queue) -> Self {
        Self {
            queue,
            queue_pos: AtomicUsize::new(0),
            finished: AtomicBool::new(false),
        }
    }
}

impl<Queue> Default for Fence<Queue>
where
    Queue: AsRef<[FenceWaker]> + Default,
{
    fn default() -> Self {
        Self::new(Queue::default())
    }
}

impl<Queue> Drop for Fence<Queue>
where
    Queue: AsRef<[FenceWaker]>,
{
    fn drop(&mut self) {
        // Cleans up any wakers left over by a fence holder
        let queue_past_end = self.queue_pos.load(Ordering::Acquire);
        if queue_past_end > 0 {
            for entry in &self.queue.as_ref()[0..queue_past_end] {
                let mut local_entry = MaybeUninit::uninit();
                core::mem::swap(entry.lock().deref_mut(), &mut local_entry);
                let local_entry = unsafe { local_entry.assume_init() };
                local_entry.wake();
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct FenceHolder<'a> {
    queue: &'a [FenceWaker],
    queue_pos: &'a AtomicUsize,
    finished: &'a AtomicBool,
}

impl<Queue> Fence<Queue>
where
    Queue: AsRef<[FenceWaker]>,
{
    pub fn hold<'a>(&'a self) -> FenceHolder<'a>
    where
        Queue: 'a,
    {
        FenceHolder {
            queue: self.queue.as_ref(),
            queue_pos: &self.queue_pos,
            finished: &self.finished,
        }
    }
}

impl Drop for FenceHolder<'_> {
    fn drop(&mut self) {
        self.finished.store(true, Ordering::Release);

        // Zeroing out the queue pointer makes any future FenceHolders skip
        let queue_past_end = self.queue_pos.swap(0, Ordering::AcqRel);
        if queue_past_end > 0 {
            for entry in &self.queue[0..queue_past_end] {
                let mut local_entry = MaybeUninit::uninit();
                core::mem::swap(entry.lock().deref_mut(), &mut local_entry);
                let local_entry = unsafe { local_entry.assume_init() };
                local_entry.wake();
            }
        }
    }
}

#[derive(Debug)]
pub struct FenceWaiter<'a> {
    state: FenceWaiterState<'a>,
    finished: &'a AtomicBool,
}

#[derive(Debug)]
pub enum FenceWaiterState<'a> {
    Uninitialized {
        queue: &'a [FenceWaker],
        queue_pos: &'a AtomicUsize,
    },
    Waiting {
        waker: &'a FenceWaker,
    },
    Overcapacity,
}

impl<Queue> Fence<Queue>
where
    Queue: AsRef<[FenceWaker]>,
{
    pub fn wait<'a>(&'a self) -> FenceWaiter<'a>
    where
        Queue: 'a,
    {
        FenceWaiter {
            finished: &self.finished,
            state: FenceWaiterState::Uninitialized {
                queue: self.queue.as_ref(),
                queue_pos: &self.queue_pos,
            },
        }
    }
}

impl Future for FenceWaiter<'_> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.finished.load(Ordering::Acquire) {
            Poll::Ready(())
        } else {
            match self.state {
                FenceWaiterState::Overcapacity => {
                    cx.waker().wake_by_ref();
                }
                FenceWaiterState::Uninitialized { queue, queue_pos } => {
                    // Implement pos + 1 logic without any overflow risks.
                    // Never fills the last element of a usize::MAX array.
                    // That is the cost of using a usize::MAX array.
                    let resolved_pos = {
                        let mut loaded_pos = queue_pos.load(Ordering::Acquire);
                        while loaded_pos < queue.len()
                            && queue_pos
                                .compare_exchange(
                                    loaded_pos,
                                    loaded_pos + 1,
                                    Ordering::AcqRel,
                                    Ordering::Acquire,
                                )
                                .is_err()
                        {
                            loaded_pos += 1;
                        }
                        loaded_pos
                    };

                    self.state = if resolved_pos < queue.len() {
                        // Initialize the waker with the current context
                        let waker = &queue[resolved_pos];
                        {
                            let mut waker_lock = waker.lock();

                            // Rollback the counter, avoid initializing, and
                            // return early if finished.
                            if self.finished.load(Ordering::Acquire) {
                                // Correct the counter for the later `Fence` drop.
                                let _ = queue_pos.fetch_sub(1, Ordering::Release);

                                return Poll::Ready(());
                            }

                            *waker_lock = MaybeUninit::new(cx.waker().clone());
                        }

                        FenceWaiterState::Waiting { waker }
                    } else {
                        cx.waker().wake_by_ref();
                        FenceWaiterState::Overcapacity
                    };
                }
                FenceWaiterState::Waiting { waker } => {
                    if let Some(mut waker) = waker.try_lock() {
                        let waker = waker.deref_mut();
                        // The state MUST have been uninitialized first, and
                        // then initialized the waker before going to this state
                        let waker = unsafe { waker.assume_init_mut() };
                        waker.clone_from(cx.waker());
                    } else {
                        // If the lock is being held, it is likely that
                        // the holder is mid-drop and changed `finished`
                        return self.poll(cx);
                    }
                }
            }

            Poll::Pending
        }
    }
}
