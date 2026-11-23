pub const HANDLER_THROW: &str = "export function handle(request) { if (new URL(request.url).pathname === '/throw') throw new Error('handler exploded'); return new Response('warm'); }";

pub const NAMED_BUNDLES: &[(&str, &str)] = &[
    (
        "orders",
        "export function handle() { return new Response('orders'); }",
    ),
    (
        "users",
        "export function handle() { return new Response('users'); }",
    ),
];
