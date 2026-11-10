//! 消息分发链路上的 Resolver 运行时与限流。

mod dispatch_rate_limit;
mod recipient_runtime;

pub use dispatch_rate_limit::DispatchRateLimiter;
pub use recipient_runtime::RecipientResolverRuntime;
