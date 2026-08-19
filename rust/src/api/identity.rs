//! Core API v0 — identity area. The local persona (name/colour), edited here.

use super::types::{PersonaColourDto, PersonaDto};

/// The local persona.
pub fn current_persona() -> Result<PersonaDto, String> {
    crate::engine::current_persona()
}

/// Update the local persona's name and colour, bumping its version.
pub fn update_persona(name: String, colour: u32) -> Result<PersonaDto, String> {
    crate::engine::update_persona(name, colour)
}

/// The colours offered on first launch.
///
/// Served from the core rather than listed again in the UI, because there are
/// already two places that must agree about a colour — what is drawn for a new
/// device, and what is offered when they change it — and a third copy in Dart
/// would be the one nobody updates. Any colour is still accepted by
/// [`update_persona`]; this is the palette, not a whitelist.
pub fn persona_colours() -> Vec<PersonaColourDto> {
    crate::engine::persona_colours()
        .into_iter()
        .map(|(name, value)| PersonaColourDto {
            name: name.to_owned(),
            value,
        })
        .collect()
}
