//! Contact cards (vCard 3.0) for the vCard tab.

use std::fmt;

/// The fields on the vCard tab. Values are trimmed when the card is built.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Contact {
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub mobile: String,
    pub work_phone: String,
    pub company: String,
    pub title: String,
    pub website: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VCardError {
    MissingName,
    /// The named phone field holds characters other than digits and `+-() `.
    InvalidPhone(&'static str),
}

impl fmt::Display for VCardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VCardError::MissingName => f.write_str("First and last name are required."),
            VCardError::InvalidPhone(field) => {
                write!(f, "The {field} number may only contain digits, spaces and + - ( ).")
            }
        }
    }
}

impl std::error::Error for VCardError {}

/// Characters allowed in phone fields: digits (any script, like Python's
/// `isdigit`) and `+-() `. The Python GUI enforced this per keystroke; the
/// core now enforces it too.
pub fn is_valid_phone(value: &str) -> bool {
    value.chars().all(|c| c.is_numeric() || "+-() ".contains(c))
}

/// Escape a text value: backslash, semicolon, comma and newline carry
/// structure in vCard. Carriage returns and other control characters are
/// dropped so pasted text cannot start a new property line.
fn escape_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// URLs are a URI value, not text: escaping their commas would corrupt them.
/// Only line breaks and control characters are removed.
fn clean_uri(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).collect()
}

impl Contact {
    /// Build the vCard text, with CRLF line endings. Blank optional fields are
    /// left out.
    pub fn to_vcard(&self) -> Result<String, VCardError> {
        let first = self.first_name.trim();
        let last = self.last_name.trim();
        if first.is_empty() || last.is_empty() {
            return Err(VCardError::MissingName);
        }
        let mobile = self.mobile.trim();
        let work = self.work_phone.trim();
        if !is_valid_phone(mobile) {
            return Err(VCardError::InvalidPhone("mobile"));
        }
        if !is_valid_phone(work) {
            return Err(VCardError::InvalidPhone("work"));
        }

        let (first, last) = (escape_text(first), escape_text(last));
        let mut lines = vec![
            "BEGIN:VCARD".to_string(),
            "VERSION:3.0".to_string(),
            format!("N:{last};{first};;;"),
            format!("FN:{first} {last}"),
        ];
        type Encode = fn(&str) -> String;
        let optional: [(&str, &str, Encode); 6] = [
            ("ORG:", self.company.trim(), escape_text),
            ("TITLE:", self.title.trim(), escape_text),
            ("TEL;TYPE=CELL:", mobile, escape_text),
            ("TEL;TYPE=WORK:", work, escape_text),
            ("EMAIL;TYPE=INTERNET,WORK:", self.email.trim(), escape_text),
            ("URL:", self.website.trim(), clean_uri),
        ];
        for (prefix, value, encode) in optional {
            if !value.is_empty() {
                lines.push(format!("{prefix}{}", encode(value)));
            }
        }
        lines.push("END:VCARD".to_string());
        Ok(lines.join("\r\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_python_output() {
        let c = Contact {
            first_name: " Ann ".into(),
            last_name: "O'Neil; Jr".into(),
            email: "ann@example.com".into(),
            mobile: "+1 (555) 010-0000".into(),
            company: "Flooring Partners, LLC".into(),
            website: "https://example.com/a,b".into(),
            ..Contact::default()
        };
        assert_eq!(
            c.to_vcard().unwrap(),
            "BEGIN:VCARD\r\nVERSION:3.0\r\nN:O'Neil\\; Jr;Ann;;;\r\nFN:Ann O'Neil\\; Jr\r\nORG:Flooring Partners\\, LLC\r\n\
             TEL;TYPE=CELL:+1 (555) 010-0000\r\nEMAIL;TYPE=INTERNET,WORK:ann@example.com\r\nURL:https://example.com/a,b\r\nEND:VCARD"
        );
    }

    #[test]
    fn rejects_bad_input_and_injection() {
        assert_eq!(Contact::default().to_vcard(), Err(VCardError::MissingName));
        let mut c = Contact { first_name: "A".into(), last_name: "B".into(), ..Contact::default() };
        c.mobile = "555-0100 ext 2".into();
        assert_eq!(c.to_vcard(), Err(VCardError::InvalidPhone("mobile")));
        c.mobile.clear();
        c.title = "Boss\r\nURL:https://evil.example".into();
        assert!(c.to_vcard().unwrap().contains("TITLE:Boss\\nURL:https://evil.example\r\n"));
        c.website = "https://ok.example\r\nNOTE:x".into();
        assert!(c.to_vcard().unwrap().contains("URL:https://ok.exampleNOTE:x\r\n"));
    }
}
