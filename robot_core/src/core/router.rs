/// Event routing system for flexible input-output pairing
/// Supports 1-to-1, 1-to-many, many-to-1 routing patterns
use crate::utils::InputEvent;
use std::collections::HashMap;

/// Marker trait for handler types
pub trait HandlerMarker: 'static + Send + Sync {
    const ID: &'static str;
}

/// Handler ID that is type-safe and compile-time verified
#[derive(Debug, Clone, Eq, Hash, PartialEq)]
pub struct HandlerId {
    type_id: std::any::TypeId,
    name: &'static str,
}

impl HandlerId {
    pub fn of<T: HandlerMarker>() -> Self {
        Self {
            type_id: std::any::TypeId::of::<T>(),
            name: T::ID,
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }
}

/// Route configuration defining which inputs go to which outputs
#[derive(Debug, Clone)]
pub struct Route {
    pub from: HandlerId,
    pub to: Vec<HandlerId>,
}

impl Route {
    pub fn new(from: HandlerId, to: HandlerId) -> Self {
        Self { from, to: vec![to] }
    }

    pub fn add_output(mut self, to: HandlerId) -> Self {
        self.to.push(to);
        self
    }
}

/// Router manages the mapping between input and output handlers
pub struct EventRouter {
    routes: HashMap<HandlerId, Vec<HandlerId>>,
    source_routes: HashMap<std::any::TypeId, Vec<HandlerId>>,
    source_name_map: HashMap<String, std::any::TypeId>,
}

impl EventRouter {
    pub fn new() -> Self {
        Self {
            routes: HashMap::new(),
            source_routes: HashMap::new(),
            source_name_map: HashMap::new(),
        }
    }

    /// Add a route from input handler to one or more output handlers
    pub fn add_route(&mut self, route: Route) {
        self.routes
            .entry(route.from.clone())
            .or_insert_with(Vec::new)
            .extend(route.to.clone());
    }

    /// Add a type-safe route from input source marker to output handlers
    pub fn add_source_route<S: HandlerMarker>(&mut self, outputs: Vec<HandlerId>) {
        let type_id = std::any::TypeId::of::<S>();
        self.source_routes
            .entry(type_id)
            .or_insert_with(Vec::new)
            .extend(outputs);
        // Register the source name for string-based lookup
        self.source_name_map.insert(S::ID.to_string(), type_id);
    }

    /// Get output handlers for a given input handler ID
    pub fn get_outputs_for_handler(&self, handler_id: &HandlerId) -> Option<&[HandlerId]> {
        self.routes.get(handler_id).map(|v| v.as_slice())
    }

    /// Get output handlers for a given input source type ID
    pub fn get_outputs_for_source_type(
        &self,
        source_type: std::any::TypeId,
    ) -> Option<&[HandlerId]> {
        self.source_routes.get(&source_type).map(|v| v.as_slice())
    }

    /// Get output handlers for an event
    /// First tries by source name, then by handler ID
    pub fn get_outputs_for_event(&self, event: &InputEvent) -> Vec<HandlerId> {
        // Try source-based routing by looking up source name in map
        if let Some(type_id) = self.source_name_map.get(&event.source) {
            if let Some(outputs) = self.source_routes.get(type_id) {
                return outputs.to_vec();
            }
        }

        // Empty routes means broadcast to all (default behavior)
        Vec::new()
    }
    /// Check if this router has any routes configured
    pub fn has_routes(&self) -> bool {
        !self.routes.is_empty() || !self.source_routes.is_empty()
    }
}

impl Default for EventRouter {
    fn default() -> Self {
        Self::new()
    }
}
