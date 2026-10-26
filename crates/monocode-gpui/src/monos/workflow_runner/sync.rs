//! The small single-threaded async pieces the run executor is built from: a one-shot signal, a
//! reusable wake-up, a first-in-first-out gate and a two-way race. Everything runs on the
//! foreground executor, so plain `Rc`/`Cell` suffice.
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    future::{Future, poll_fn},
    pin::{Pin, pin},
    rc::Rc,
    task::{Poll, Waker},
};

fn remember(wakers: &mut Vec<Waker>, waker: &Waker) {
    if !wakers.iter().any(|known| known.will_wake(waker)) {
        wakers.push(waker.clone());
    }
}

#[derive(Default)]
struct SignalState {
    fired: Cell<bool>,
    wakers: RefCell<Vec<Waker>>,
}

/// Fires once and releases every waiter, present and future (the TS `stopSignal` promise).
#[derive(Clone, Default)]
pub(super) struct Signal(Rc<SignalState>);

impl Signal {
    pub(super) fn fire(&self) {
        if !self.0.fired.replace(true) {
            for waker in self.0.wakers.take() {
                waker.wake();
            }
        }
    }
    pub(super) fn wait(&self) -> impl Future<Output = ()> + use<> {
        let state = self.0.clone();
        poll_fn(move |cx| {
            if state.fired.get() {
                Poll::Ready(())
            } else {
                remember(&mut state.wakers.borrow_mut(), cx.waker());
                Poll::Pending
            }
        })
    }
}

#[derive(Default)]
struct NudgeState {
    set: Cell<bool>,
    waker: RefCell<Option<Waker>>,
}

/// A wake-up that is remembered until someone waits for it (the TS `nudged` flag plus `wake`).
/// One waiter at a time.
#[derive(Clone, Default)]
pub(super) struct Nudge(Rc<NudgeState>);

impl Nudge {
    pub(super) fn nudge(&self) {
        self.0.set.set(true);
        if let Some(waker) = self.0.waker.take() {
            waker.wake();
        }
    }
    pub(super) fn wait(&self) -> impl Future<Output = ()> + use<> {
        let state = self.0.clone();
        poll_fn(move |cx| {
            if state.set.replace(false) {
                Poll::Ready(())
            } else {
                *state.waker.borrow_mut() = Some(cx.waker().clone());
                Poll::Pending
            }
        })
    }
}

#[derive(Default)]
struct GateState {
    held: Cell<bool>,
    queue: RefCell<VecDeque<(u64, Waker)>>,
    next: Cell<u64>,
}

/// An async mutex that hands out turns in the order they were asked for (the TS `serial` chain).
#[derive(Clone, Default)]
pub(super) struct Gate(Rc<GateState>);

pub(super) struct Turn(Rc<GateState>);

impl Drop for Turn {
    fn drop(&mut self) {
        self.0.held.set(false);
        if let Some((_, waker)) = self.0.queue.borrow().front() {
            waker.wake_by_ref();
        }
    }
}

impl Gate {
    pub(super) async fn turn(&self) -> Turn {
        let state = self.0.clone();
        let ticket = state.next.get();
        state.next.set(ticket + 1);
        // Leaves the queue whether it got its turn or was dropped while waiting.
        struct Waiting(Rc<GateState>, u64);
        impl Drop for Waiting {
            fn drop(&mut self) {
                let mut queue = self.0.queue.borrow_mut();
                queue.retain(|(ticket, _)| *ticket != self.1);
                if let Some((_, waker)) = queue.front() {
                    waker.wake_by_ref();
                }
            }
        }
        let waiting = Waiting(state.clone(), ticket);
        poll_fn(|cx| {
            let mut queue = state.queue.borrow_mut();
            match queue.iter_mut().find(|(t, _)| *t == ticket) {
                Some(entry) => entry.1 = cx.waker().clone(),
                None => queue.push_back((ticket, cx.waker().clone())),
            }
            if !state.held.get() && queue.front().is_some_and(|(t, _)| *t == ticket) {
                state.held.set(true);
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        drop(waiting);
        Turn(state)
    }
}

pub(super) enum Either<A, B> {
    Left(A),
    Right(B),
}

/// Whichever finishes first; the left one wins a tie. The other is dropped.
pub(super) async fn race<A: Future, B: Future>(a: A, b: B) -> Either<A::Output, B::Output> {
    let (mut a, mut b) = (pin!(a), pin!(b));
    poll_fn(move |cx| {
        if let Poll::Ready(value) = a.as_mut().poll(cx) {
            return Poll::Ready(Either::Left(value));
        }
        if let Poll::Ready(value) = b.as_mut().poll(cx) {
            return Poll::Ready(Either::Right(value));
        }
        Poll::Pending
    })
    .await
}

/// Runs the futures side by side until all are done.
pub(super) async fn join_all(futures: Vec<Pin<Box<dyn Future<Output = ()>>>>) {
    let mut futures: Vec<Option<Pin<Box<dyn Future<Output = ()>>>>> =
        futures.into_iter().map(Some).collect();
    poll_fn(move |cx| {
        let mut pending = false;
        for slot in &mut futures {
            if let Some(future) = slot
                && future.as_mut().poll(cx).is_ready()
            {
                *slot = None;
            }
            pending |= slot.is_some();
        }
        if pending {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    })
    .await;
}
