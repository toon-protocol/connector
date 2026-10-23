# 2. Put your app behind it

A route with a `handler_url` **terminates** there. The connector opens the sealed
payload, makes exactly that HTTP request of your app, and seals the app's
complete response back.

Your app is payment-oblivious. It receives an ordinary HTTP request, holds no key
on your behalf, and supplies nothing toward the packet's fulfilment — the
connector derives that itself. So "the app answered" and "the packet was paid
for" stay separable, and an app that knows nothing about payment cannot leak,
forge or withhold one.

The one thing this connector adds is attribution, on a request it took the
payment for itself: `X-TOON-Payer` (the paying channel), `X-TOON-Amount` (what
that request was charged) and `X-TOON-Chain`. Your app is free to ignore all
three — it is handed them so it can log or rate-limit by payer if it wants to,
not so it can decide anything about the payment. They are absent on a request
this node did not take the payment for, so treat them as optional. Whatever a
caller writes under those names is stripped before your app sees it.

Two things decide what you can charge for:

- **You are paid for an answer, not the answer the caller wanted.** A `404` from
  your app is a real answer: it rides home on a `FULFILL` and costs the same as a
  `200`. Only unreachability or a refused target produces a reject — and one
  exception you opt into: a request larger than the route's `max_bytes` is refused
  `F00` before it is paid for and never reaches your app, so a body your app would
  only answer `413` costs the caller nothing. `max_bytes` measures the sealed
  packet, which is longer than the body your app receives: set it to your app's
  own limit and a body just under that limit is refused; set it higher and a body
  just over it still reaches your app's `413`. Clients read it as `maxBytes`
  beside the route's price.
- **The trailing slash on `handler_url` is load-bearing**, and a request's target
  is resolved _beneath_ the handler's path — an absolute path, a `..` segment, a
  scheme or an authority is refused before your app is touched.

A terminated route **must** carry a `price`. Write `price = 0` if free is
deliberate, because it is never silently free.
