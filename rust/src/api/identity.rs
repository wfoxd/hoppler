//! Core API v0 — identity area. The local persona (name/colour), edited here.

use super::types::PersonaDto;

/// The local persona.
pub fn current_persona() -> Result<PersonaDto, String> {
    crate::engine::current_persona()
}

/// Update the local persona's name and colour, bumping its version.
pub fn update_persona(name: String, colour: u32) -> Result<PersonaDto, String> {
    crate::engine::update_persona(name, colour)
}
