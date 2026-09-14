/// Backing that outlives an endpoint: mapping, completion cells, and receiver records.
pub mod retained;
/// Payload-pool ring: one direction of the transport.
pub mod ring;
mod sys;
