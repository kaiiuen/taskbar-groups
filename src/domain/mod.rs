//! Pure taskbar-group domain models and legacy XML compatibility helpers.

use std::fmt;

pub const MAX_SHORTCUTS: usize = 20;
pub const DEFAULT_COLOR: &str = "#1f1f1f";
pub const DEFAULT_OPACITY: f64 = 10.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Category {
    pub name: String,
    pub color_string: String,
    pub allow_open_all: bool,
    pub shortcut_list: Vec<ProgramShortcut>,
    pub width: i32,
    pub opacity: f64,
    /// Optional group icon selection retained in the legacy-compatible config.
    pub icon_source: Option<GroupIconSource>,
}

impl Default for Category {
    fn default() -> Self {
        Self {
            name: String::new(),
            color_string: DEFAULT_COLOR.to_owned(),
            allow_open_all: false,
            shortcut_list: Vec::new(),
            width: 0,
            opacity: DEFAULT_OPACITY,
            icon_source: None,
        }
    }
}

impl Category {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.name.is_empty() {
            return Err(ValidationError::EmptyCategoryName);
        }
        if !self
            .name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == ' ')
        {
            return Err(ValidationError::InvalidCategoryName);
        }
        if self.shortcut_list.is_empty() {
            return Err(ValidationError::NoShortcuts);
        }
        if self.shortcut_list.len() > MAX_SHORTCUTS {
            return Err(ValidationError::TooManyShortcuts {
                maximum: MAX_SHORTCUTS,
            });
        }
        if !self.opacity.is_finite() || !(0.0..=100.0).contains(&self.opacity) {
            return Err(ValidationError::InvalidOpacity);
        }
        if self.width < 0 {
            return Err(ValidationError::InvalidWidth);
        }
        for (index, shortcut) in self.shortcut_list.iter().enumerate() {
            shortcut
                .validate()
                .map_err(|source| ValidationError::InvalidShortcut {
                    index,
                    source: Box::new(source),
                })?;
        }
        Ok(())
    }

    /// Serialize using the element names emitted by .NET `XmlSerializer`.
    pub fn to_legacy_xml(&self) -> String {
        let shortcuts = self
            .shortcut_list
            .iter()
            .map(ProgramShortcut::to_legacy_xml)
            .collect::<String>();
        let icon = self.icon_source.as_ref().map_or_else(String::new, |icon| {
            format!(
                "<IconPath>{}</IconPath><IconIndex>{}</IconIndex>",
                escape_xml(&icon.path),
                icon.index
            )
        });
        format!(
            "<Category><Name>{}</Name><ColorString>{}</ColorString><allowOpenAll>{}</allowOpenAll><ShortcutList>{}</ShortcutList><Width>{}</Width><Opacity>{}</Opacity>{}</Category>",
            escape_xml(&self.name), escape_xml(&self.color_string), self.allow_open_all,
            shortcuts, self.width, self.opacity, icon
        )
    }

    /// Read the simple legacy document shape produced by `XmlSerializer`.
    /// Unknown elements are ignored, matching the serializer's default behavior.
    pub fn from_legacy_xml(xml: &str) -> Result<Self, XmlError> {
        let mut category = Self::default();
        category.name = required_text(xml, "Name")?;
        if let Some(value) = optional_text(xml, "ColorString")? {
            category.color_string = value;
        }
        if let Some(value) = optional_text(xml, "allowOpenAll")? {
            category.allow_open_all = parse_bool(&value, "allowOpenAll")?;
        }
        if let Some(value) = optional_text(xml, "Width")? {
            category.width = value.parse().map_err(|_| XmlError::InvalidValue("Width"))?;
        }
        if let Some(value) = optional_text(xml, "Opacity")? {
            category.opacity = value
                .parse()
                .map_err(|_| XmlError::InvalidValue("Opacity"))?;
        }
        if let Some(path) = optional_text(xml, "IconPath")? {
            let index = optional_text(xml, "IconIndex")?
                .map(|value| {
                    value
                        .parse()
                        .map_err(|_| XmlError::InvalidValue("IconIndex"))
                })
                .transpose()?
                .unwrap_or(0);
            category.icon_source = Some(GroupIconSource { path, index });
        }
        if let Some(shortcut_list) = raw_element_text(xml, "ShortcutList")? {
            let mut remaining = shortcut_list.as_str();
            while let Some(start) = remaining.find("<ProgramShortcut>") {
                let after_start = &remaining[start + "<ProgramShortcut>".len()..];
                let end = after_start
                    .find("</ProgramShortcut>")
                    .ok_or(XmlError::Malformed("ProgramShortcut"))?;
                category
                    .shortcut_list
                    .push(ProgramShortcut::from_legacy_xml(&after_start[..end])?);
                remaining = &after_start[end + "</ProgramShortcut>".len()..];
            }
        }
        Ok(category)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupIconSource {
    pub path: String,
    pub index: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramShortcut {
    pub file_path: String,
    pub is_windows_app: bool,
    pub name: String,
    pub arguments: String,
    pub working_directory: String,
}

impl Default for ProgramShortcut {
    fn default() -> Self {
        Self {
            file_path: String::new(),
            is_windows_app: false,
            name: String::new(),
            arguments: String::new(),
            working_directory: String::new(),
        }
    }
}

impl ProgramShortcut {
    pub fn new(file_path: impl Into<String>) -> Self {
        Self {
            file_path: file_path.into(),
            ..Self::default()
        }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.file_path.trim().is_empty() {
            return Err(ValidationError::EmptyShortcutPath);
        }
        Ok(())
    }

    pub fn to_legacy_xml(&self) -> String {
        format!(
            "<ProgramShortcut><FilePath>{}</FilePath><isWindowsApp>{}</isWindowsApp><name>{}</name><Arguments>{}</Arguments><WorkingDirectory>{}</WorkingDirectory></ProgramShortcut>",
            escape_xml(&self.file_path),
            self.is_windows_app,
            escape_xml(&self.name),
            escape_xml(&self.arguments),
            escape_xml(&self.working_directory)
        )
    }

    pub fn from_legacy_xml(xml: &str) -> Result<Self, XmlError> {
        let mut shortcut = Self::default();
        shortcut.file_path = required_text(xml, "FilePath")?;
        if let Some(value) = optional_text(xml, "isWindowsApp")? {
            shortcut.is_windows_app = parse_bool(&value, "isWindowsApp")?;
        }
        if let Some(value) = optional_text(xml, "name")? {
            shortcut.name = value;
        }
        if let Some(value) = optional_text(xml, "Arguments")? {
            shortcut.arguments = value;
        }
        if let Some(value) = optional_text(xml, "WorkingDirectory")? {
            shortcut.working_directory = value;
        }
        Ok(shortcut)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    EmptyCategoryName,
    InvalidCategoryName,
    NoShortcuts,
    TooManyShortcuts {
        maximum: usize,
    },
    InvalidOpacity,
    InvalidWidth,
    EmptyShortcutPath,
    InvalidShortcut {
        index: usize,
        source: Box<ValidationError>,
    },
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XmlError {
    MissingElement(&'static str),
    Malformed(&'static str),
    InvalidValue(&'static str),
}

fn required_text(xml: &str, tag: &'static str) -> Result<String, XmlError> {
    element_text(xml, tag)?.ok_or(XmlError::MissingElement(tag))
}

fn optional_text(xml: &str, tag: &'static str) -> Result<Option<String>, XmlError> {
    element_text(xml, tag)
}

fn element_text(xml: &str, tag: &'static str) -> Result<Option<String>, XmlError> {
    raw_element_text(xml, tag)?
        .map(|value| unescape_xml(&value))
        .transpose()
}

fn raw_element_text(xml: &str, tag: &'static str) -> Result<Option<String>, XmlError> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let Some(start) = xml.find(&open) else {
        return Ok(None);
    };
    let value_start = start + open.len();
    let Some(relative_end) = xml[value_start..].find(&close) else {
        return Err(XmlError::Malformed(tag));
    };
    Ok(Some(
        xml[value_start..value_start + relative_end].to_owned(),
    ))
}

fn parse_bool(value: &str, field: &'static str) -> Result<bool, XmlError> {
    match value {
        "true" | "True" | "TRUE" => Ok(true),
        "false" | "False" | "FALSE" => Ok(false),
        _ => Err(XmlError::InvalidValue(field)),
    }
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn unescape_xml(value: &str) -> Result<String, XmlError> {
    let entities = ["&lt;", "&gt;", "&quot;", "&apos;", "&amp;"];
    let mut remainder = value;
    while let Some(index) = remainder.find('&') {
        let entity = &remainder[index..];
        if !entities.iter().any(|known| entity.starts_with(known)) {
            return Err(XmlError::Malformed("entity"));
        }
        remainder =
            &remainder[index + entity.find(';').ok_or(XmlError::Malformed("entity"))? + 1..];
    }

    let mut result = value.to_owned();
    for (entity, character) in [
        ("&lt;", '<'),
        ("&gt;", '>'),
        ("&quot;", '"'),
        ("&apos;", '\''),
        ("&amp;", '&'),
    ] {
        result = result.replace(entity, &character.to_string());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_legacy_initializers() {
        let category = Category::default();
        assert_eq!(category.color_string, DEFAULT_COLOR);
        assert!(!category.allow_open_all);
        assert_eq!(category.opacity, DEFAULT_OPACITY);
        assert_eq!(ProgramShortcut::default().arguments, "");
        assert!(!ProgramShortcut::default().is_windows_app);
    }

    #[test]
    fn legacy_xml_round_trips_escaped_values() {
        let mut category = Category::new("Games");
        category.shortcut_list.push(ProgramShortcut {
            file_path: r"C:\Games & Tools\play.exe".into(),
            name: "Play <now>".into(),
            arguments: "--profile=\"default\"".into(),
            working_directory: r"C:\Games".into(),
            ..ProgramShortcut::default()
        });
        let restored = Category::from_legacy_xml(&category.to_legacy_xml()).unwrap();
        assert_eq!(restored, category);
    }

    #[test]
    fn missing_optional_legacy_fields_use_defaults() {
        let category = Category::from_legacy_xml("<Category><Name>Old</Name></Category>").unwrap();
        assert_eq!(category.color_string, DEFAULT_COLOR);
        assert_eq!(category.opacity, DEFAULT_OPACITY);
        assert!(category.shortcut_list.is_empty());
    }

    #[test]
    fn validation_enforces_ui_limits_without_ui_dependencies() {
        let mut category = Category::new("Valid Group");
        assert_eq!(category.validate(), Err(ValidationError::NoShortcuts));
        category.shortcut_list.push(ProgramShortcut::new("app.exe"));
        assert!(category.validate().is_ok());
        category.name = "bad/name".into();
        assert_eq!(
            category.validate(),
            Err(ValidationError::InvalidCategoryName)
        );
    }
}
