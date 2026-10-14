const BOM: &[u8] = b"\xEF\xBB\xBF";

/// The text without a leading BOM, line endings untouched; `None` when it is not UTF-8.
pub(crate) fn decode(bytes: &[u8]) -> Option<&str> {
    std::str::from_utf8(bytes.strip_prefix(BOM).unwrap_or(bytes)).ok()
}
