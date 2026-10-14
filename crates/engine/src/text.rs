use std::borrow::Cow;

const BOM: &[u8] = b"\xEF\xBB\xBF";

/// The text without a leading BOM, line endings untouched, and whether it had the BOM;
/// `None` when it is not UTF-8.
pub(crate) fn decode(bytes: &[u8]) -> Option<(&str, bool)> {
    let (rest, bom) = match bytes.strip_prefix(BOM) {
        Some(rest) => (rest, true),
        None => (bytes, false),
    };
    Some((std::str::from_utf8(rest).ok()?, bom))
}

pub(crate) fn encode(text: String, bom: bool) -> Vec<u8> {
    let mut bytes = text.into_bytes();
    if bom {
        bytes.splice(0..0, BOM.iter().copied());
    }
    bytes
}

/// A rule writes `\n`; the file may use `\r\n`. Both in `text` become `ending`, so a
/// `\r\n` the rule wrote is not doubled.
pub(crate) fn with_line_ending<'t>(text: &'t str, ending: &str) -> Cow<'t, str> {
    if !text.contains('\n') {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.replace("\r\n", "\n").replace('\n', ending))
}
