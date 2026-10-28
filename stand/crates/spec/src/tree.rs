//! The YAML of the stand file as a tree that keeps the line of every key: `saphyr` stays here.

use saphyr::{LoadableYamlNode, MarkedYaml, Marker, Scalar, YamlData};

use crate::{Invalid, TREE};

/// The key of what YAML itself refuses: no key of the stand file is at fault.
const YAML: &str = "yaml";

/// What a missing key holds: nothing under it is there either.
static MISSING: Node = Node {
    line: 0,
    value: Value::Null,
};

pub(crate) struct Node {
    line: u32,
    value: Value,
}

enum Value {
    Null,
    Bool(bool),
    /// A string, or a number as written: `+10`, `0.50`, `0x10`.
    Text(String),
    List(Vec<Node>),
    Map(Vec<Entry>),
    /// A tag outside the core schema, such as Compose's `!reset`.
    Tagged(String),
}

struct Entry {
    key: String,
    line: u32,
    value: Node,
}

/// The one document of `text`; an empty text is a null document.
pub(crate) fn parse(text: &str) -> Result<Node, Invalid> {
    let mut documents = MarkedYaml::load_from_str(text).map_err(|error| Invalid {
        line: line(error.marker()),
        key: YAML.to_owned(),
        message: error.info().to_owned(),
    })?;
    if let Some(second) = documents.get(1) {
        return Err(Invalid {
            line: line(&second.span.start),
            key: YAML.to_owned(),
            message: "a second document: the stand file is one".to_owned(),
        });
    }
    let source = Source::new(text);
    match documents.pop() {
        Some(document) => node(document, &source),
        None => Ok(Node {
            line: 1,
            value: Value::Null,
        }),
    }
}

/// The text `saphyr` read, which marks positions in chars.
struct Source<'t> {
    text: &'t str,
    /// The byte offset of every char, and the end.
    offsets: Vec<usize>,
}

impl<'t> Source<'t> {
    fn new(text: &'t str) -> Source<'t> {
        let offsets = text.char_indices().map(|(at, _)| at).chain([text.len()]);
        Source {
            text,
            offsets: offsets.collect(),
        }
    }

    fn between(&self, start: &Marker, end: &Marker) -> Option<String> {
        let start = *self.offsets.get(start.index())?;
        let end = *self.offsets.get(end.index())?;
        self.text.get(start..end).map(str::to_owned)
    }
}

fn node(yaml: MarkedYaml, source: &Source) -> Result<Node, Invalid> {
    let span = yaml.span;
    let line = line(&span.start);
    // YAML gives back `+10` as `10` and `0.50` as `0.5`.
    let number = |read: String| source.between(&span.start, &span.end).unwrap_or(read);
    let value = match yaml.data {
        YamlData::Value(Scalar::Null) => Value::Null,
        YamlData::Value(Scalar::Boolean(value)) => Value::Bool(value),
        YamlData::Value(Scalar::Integer(read)) => Value::Text(number(read.to_string())),
        YamlData::Value(Scalar::FloatingPoint(read)) => Value::Text(number(read.0.to_string())),
        YamlData::Value(Scalar::String(text)) => Value::Text(text.into_owned()),
        YamlData::Sequence(items) => Value::List(
            items
                .into_iter()
                .map(|item| node(item, source))
                .collect::<Result<_, _>>()?,
        ),
        YamlData::Mapping(entries) => Value::Map(
            entries
                .into_iter()
                .map(|pair| entry(pair, source))
                .collect::<Result<_, _>>()?,
        ),
        YamlData::Tagged(tag, _) => Value::Tagged(tag.to_string()),
        // `load_from_str` resolves scalars and aliases, so these are values it could not read.
        YamlData::Representation(..) | YamlData::Alias(_) | YamlData::BadValue => {
            return Err(Invalid {
                line,
                key: YAML.to_owned(),
                message: "a value YAML could not read".to_owned(),
            });
        }
    };
    Ok(Node { line, value })
}

fn entry((key, value): (MarkedYaml, MarkedYaml), source: &Source) -> Result<Entry, Invalid> {
    let span = key.span;
    let line = line(&span.start);
    let number = |read: String| source.between(&span.start, &span.end).unwrap_or(read);
    let text = match key.data {
        YamlData::Value(Scalar::String(text)) => text.into_owned(),
        YamlData::Value(Scalar::Integer(read)) => number(read.to_string()),
        YamlData::Value(Scalar::FloatingPoint(read)) => number(read.0.to_string()),
        YamlData::Value(Scalar::Boolean(value)) => value.to_string(),
        _ => {
            return Err(Invalid {
                line,
                key: YAML.to_owned(),
                message: "a key that is not a name".to_owned(),
            });
        }
    };
    Ok(Entry {
        key: text,
        line,
        value: node(value, source)?,
    })
}

fn line(marker: &Marker) -> u32 {
    u32::try_from(marker.line()).unwrap_or(u32::MAX)
}

/// `$$` is `$`. Any other `$` starts a substitution, and the stand makes none: there is no host
/// environment to take it from, and the two versions must read the same file.
pub(crate) fn interpolate(text: &str) -> Result<String, String> {
    let mut read = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        read.push_str(&rest[..at]);
        rest = rest[at + 1..].strip_prefix('$').ok_or_else(|| {
            format!(
                "`{text}` has a `$` substitution, and the stand makes none (`${{STAND_REPO}}` is \
                 only the source of a bind on {TREE}); write `$$` for a `$`"
            )
        })?;
        read.push('$');
    }
    read.push_str(rest);
    Ok(read)
}

/// A node with its key path and the line to report it at.
pub(crate) struct Field<'a> {
    node: &'a Node,
    /// Such as `services.api.volumes[0]`; empty for the document.
    key: String,
    /// Of the key, of the list item or of the document.
    line: u32,
}

impl<'a> Field<'a> {
    pub(crate) fn root(node: &'a Node) -> Field<'a> {
        Field {
            node,
            key: String::new(),
            line: node.line,
        }
    }

    pub(crate) fn invalid(&self, message: impl Into<String>) -> Invalid {
        Invalid {
            line: self.line,
            key: self.key.clone(),
            message: message.into(),
        }
    }

    pub(crate) fn get(&self, key: &str) -> Option<Field<'a>> {
        let Value::Map(entries) = &self.node.value else {
            return None;
        };
        let entry = entries.iter().find(|entry| entry.key == key)?;
        Some(self.child(entry))
    }

    /// `get`, or a missing value at the line of this node: `services.api.deploy.resources` of a
    /// service without `deploy` is reported at the line of `api:`.
    pub(crate) fn at(&self, key: &str) -> Field<'a> {
        self.get(key).unwrap_or_else(|| Field {
            node: &MISSING,
            key: self.join(key),
            line: self.line,
        })
    }

    /// Where to report item `index` of a list.
    pub(crate) fn item(&self, index: usize) -> Field<'a> {
        match &self.node.value {
            Value::List(items) if index < items.len() => self.list_item(index, &items[index]),
            _ => Field {
                node: &MISSING,
                key: format!("{}[{index}]", self.key),
                line: self.line,
            },
        }
    }

    /// A null mapping has no entries.
    pub(crate) fn entries(&self) -> Result<Vec<(&'a str, Field<'a>)>, Invalid> {
        match &self.node.value {
            Value::Map(entries) => Ok(entries
                .iter()
                .map(|entry| (entry.key.as_str(), self.child(entry)))
                .collect()),
            Value::Null => Ok(Vec::new()),
            _ => Err(self.expected("a mapping")),
        }
    }

    /// A null list has no items.
    pub(crate) fn items(&self) -> Result<Vec<Field<'a>>, Invalid> {
        match &self.node.value {
            Value::List(items) => Ok(items
                .iter()
                .enumerate()
                .map(|(index, node)| self.list_item(index, node))
                .collect()),
            Value::Null => Ok(Vec::new()),
            _ => Err(self.expected("a list")),
        }
    }

    pub(crate) fn is_map(&self) -> bool {
        matches!(self.node.value, Value::Map(_))
    }

    pub(crate) fn is_list(&self) -> bool {
        matches!(self.node.value, Value::List(_))
    }

    pub(crate) fn is_null(&self) -> bool {
        matches!(self.node.value, Value::Null)
    }

    /// A string as written, before `interpolate`.
    pub(crate) fn raw(&self) -> Result<&'a str, Invalid> {
        match &self.node.value {
            Value::Text(text) => Ok(text),
            _ => Err(self.expected("a string")),
        }
    }

    /// A string after `interpolate`; a boolean reads `true` or `false`, as Compose has it.
    pub(crate) fn string(&self) -> Result<String, Invalid> {
        match &self.node.value {
            Value::Text(text) => interpolate(text).map_err(|message| self.invalid(message)),
            Value::Bool(value) => Ok(value.to_string()),
            _ => Err(self.expected("a string")),
        }
    }

    /// The `string` through `parse`, whose `Err` is the message.
    pub(crate) fn parse<T>(
        &self,
        parse: impl FnOnce(&str) -> Result<T, String>,
    ) -> Result<T, Invalid> {
        parse(&self.string()?).map_err(|message| self.invalid(message))
    }

    pub(crate) fn strings(&self) -> Result<Vec<String>, Invalid> {
        self.items()?.iter().map(Field::string).collect()
    }

    /// Only an unquoted `true` or `false`.
    pub(crate) fn bool(&self) -> Result<bool, Invalid> {
        match self.node.value {
            Value::Bool(value) => Ok(value),
            _ => Err(self.expected("`true` or `false`")),
        }
    }

    fn expected(&self, what: &str) -> Invalid {
        let found = match &self.node.value {
            Value::Null => "nothing".to_owned(),
            Value::Bool(value) => format!("`{value}`"),
            Value::Text(text) => format!("`{text}`"),
            Value::List(_) => "a list".to_owned(),
            Value::Map(_) => "a mapping".to_owned(),
            Value::Tagged(tag) => format!("the tag `{tag}`, and YAML tags are not supported"),
        };
        self.invalid(format!("expected {what}, found {found}"))
    }

    fn child(&self, entry: &'a Entry) -> Field<'a> {
        Field {
            node: &entry.value,
            key: self.join(&entry.key),
            line: entry.line,
        }
    }

    fn list_item(&self, index: usize, node: &'a Node) -> Field<'a> {
        Field {
            node,
            key: format!("{}[{index}]", self.key),
            line: node.line,
        }
    }

    fn join(&self, key: &str) -> String {
        if self.key.is_empty() {
            key.to_owned()
        } else {
            format!("{}.{key}", self.key)
        }
    }
}
