//! What the two parsers of dictionary files, [`oewn`](crate::oewn) and
//! [`freedict`](crate::freedict), do the same way: the error they return,
//! reading one event of the stream, reading an attribute, and collecting the
//! text of an element from its text, CDATA and references.
//!
//! What a refusal says first, which format the file is not, is the parser's:
//! each gives its own `malformed` to the functions here that may refuse.

use std::{borrow::Cow, fmt, io::BufRead};

use quick_xml::{
    Reader, XmlVersion,
    encoding::EncodingError,
    escape::resolve_predefined_entity,
    events::{BytesRef, BytesStart, Event},
};

/// Why a dictionary file could not be read; the text says what and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(String);

impl ParseError {
    pub(crate) fn new(text: String) -> Self {
        Self(text)
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

/// How a parser words a refusal: it puts the format the file is not before
/// `what`.
pub(crate) type Malformed = fn(String) -> ParseError;

/// The next event of the stream, read into `buffer`.
pub(crate) fn read_event<'b>(
    reader: &mut Reader<impl BufRead>,
    buffer: &'b mut Vec<u8>,
) -> Result<Event<'b>, ParseError> {
    reader.read_event_into(buffer).map_err(|error| {
        ParseError(format!(
            "not well-formed XML at byte {}: {error}",
            reader.error_position()
        ))
    })
}

/// The value of the attribute `name`, with its entities resolved.
pub(crate) fn attribute(
    element: &BytesStart<'_>,
    name: &str,
    malformed: Malformed,
) -> Result<Option<String>, ParseError> {
    let bad = |error: &dyn fmt::Display| {
        malformed(format!(
            "a bad attribute in {}: {error}",
            String::from_utf8_lossy(element.name().as_ref())
        ))
    };
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| bad(&error))?;
        if attribute.key.as_ref() == name.as_bytes() {
            return attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .map(|value| Some(value.into_owned()))
                .map_err(|error| bad(&error));
        }
    }
    Ok(None)
}

fn not_text(malformed: Malformed) -> impl Fn(EncodingError) -> ParseError {
    move |error| malformed(format!("a text cannot be decoded: {error}"))
}

/// Appends the `content` of a text or of a CDATA section to `target`.
pub(crate) fn push_content(
    target: &mut String,
    content: Result<Cow<'_, str>, EncodingError>,
    malformed: Malformed,
) -> Result<(), ParseError> {
    target.push_str(&content.map_err(not_text(malformed))?);
    Ok(())
}

/// Appends what `reference` stands for to `target`: the character of a
/// character reference or the text of a predefined entity. Any other entity
/// is refused with `unknown`.
pub(crate) fn push_reference(
    target: &mut String,
    reference: &BytesRef<'_>,
    malformed: Malformed,
    unknown: &str,
) -> Result<(), ParseError> {
    let unknown = || malformed(unknown.to_owned());
    match reference.resolve_char_ref().map_err(|_| unknown())? {
        Some(character) => target.push(character),
        None => {
            let name = reference.decode().map_err(not_text(malformed))?;
            target.push_str(resolve_predefined_entity(&name).ok_or_else(unknown)?);
        }
    }
    Ok(())
}
