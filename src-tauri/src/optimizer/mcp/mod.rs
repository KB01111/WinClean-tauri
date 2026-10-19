//! Windows MCP integration (preview platform feature).
//!
//! * [`registry`] parses what the On-device Agent Registry reports, handling
//!   each documented output shape and validating server identity;
//! * [`odr`] finds `odr.exe` in a protected location and runs it;
//! * [`transport`] / [`process`] / [`client`] speak MCP over a contained,
//!   bounded, always-cleaned-up child process;
//! * [`policy`] / [`schema`] decide what may be called and validate calls
//!   against the connector's own schema;
//! * [`search`] treats search results as untrusted input;
//! * [`service`] ties them together and reports *how far* the integration got:
//!   discovered → identity verified → connected → tools listed → permission
//!   granted → a tool actually executed.
//!
//! WinClean's native scanner is untouched: MCP only adds optional discovery.

pub mod client;
pub mod odr;
pub mod policy;
pub mod process;
pub mod registry;
pub mod roots;
pub mod schema;
pub mod search;
pub mod service;
pub mod transport;

pub use odr::MIN_ODR_BUILD;
pub use policy::{classify_tool, ToolPolicy};
pub use service::{McpManager, McpStatus};
