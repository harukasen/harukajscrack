//! Bundle detection and conservative module extraction primitives.
//! The detector accepts common webpack/browserify wrappers and returns module
//! source ranges without executing the bundle.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedBundle {
    pub kind: String,
    pub entry_id: String,
    pub source: String,
}

pub fn detect(source: &str) -> Option<DetectedBundle> {
    if source.contains("__webpack_modules__")
        || source.contains("webpackJsonp")
        || source.contains("webpackBootstrap")
        || source.contains("__webpack_require__")
    {
        return Some(DetectedBundle {
            kind: "webpack".into(),
            entry_id: "0".into(),
            source: source.into(),
        });
    }
    if source.contains("function(require,module,exports)")
        || source.contains("function (require, module, exports)")
        || source.contains("browserify")
    {
        return Some(DetectedBundle {
            kind: "browserify".into(),
            entry_id: "0".into(),
            source: source.into(),
        });
    }
    None
}

pub fn module_path(id: &str, entry: bool) -> String {
    if entry {
        "./index.js".into()
    } else {
        format!("./{}.js", id.replace(['/', '\\'], "_"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_webpack() {
        assert_eq!(
            detect("var __webpack_modules__ = {}; ").unwrap().kind,
            "webpack"
        );
    }
    #[test]
    fn detects_browserify() {
        assert_eq!(
            detect("function(require,module,exports){}").unwrap().kind,
            "browserify"
        );
    }
}
