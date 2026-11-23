//! The one exception every failure crosses into Python as, and the GIL release
//! every computing call runs inside.
//!
//! A Python caller never sees a Rust error type. Every failure this module raises
//! is [`OpportuneError`], which carries the stable `code`, the `message`, the
//! RFC 6901 `path` of the field at fault, and the request `errors` a rejected
//! request was found to have — so an application's exception handler can answer a
//! client with the exact pointer into the document it sent, rather than with a
//! Python-facing restatement of a Rust type name.

use opportune_json::JsonError;
use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyString};

create_exception!(
    opportune._opportune,
    OpportuneError,
    PyException,
    "A rejected request, document or engine configuration.\n\n\
     Attributes:\n\
     \x20   code (str): The stable, machine-readable code, such as `invalid_json`\n\
     \x20       or `instant_not_aligned`.\n\
     \x20   message (str): A sentence naming the value that was rejected. The text\n\
     \x20       may be reworded; match on `code` instead.\n\
     \x20   path (str | None): The RFC 6901 JSON pointer of the field at fault, or\n\
     \x20       `None` when the failure is about the document as a whole.\n\
     \x20   errors (list[dict]): The findings a rejected request carries, each with\n\
     \x20       `code`, `message` and a `path` that is `None` when the finding is\n\
     \x20       not about one field."
);

/// The code an `_Engine` construction raises when it is given a version label
/// with no directory to label.
///
/// The label names a database directory's version; the bundled database reports
/// its own, so accepting the label would silently answer with a version the
/// caller did not ask for.
pub(crate) const VERSION_WITHOUT_DIRECTORY: &str = "tzdb_version_without_directory";

/// The code a non-document string argument raises when it has no UTF-8 spelling.
///
/// A zone name or a rule ID is not a JSON document, so `invalid_json` would be
/// the wrong answer: the argument itself cannot be read, whatever it names.
pub(crate) const ARGUMENT_NOT_UTF8: &str = "argument_not_utf8";

/// The text a JSON document argument holds, read as the UTF-8 the wire layer
/// parses.
///
/// A Python `str` is a sequence of code points and a JSON document is UTF-8, so a
/// string holding a lone surrogate has no spelling this boundary can read:
/// `json.loads` produces exactly that from a client document with an unpaired
/// `\udXXX` escape, and a caller that re-serialises it without ASCII escaping
/// hands one here. `PyO3`'s own `&str` extraction raises `UnicodeEncodeError` for
/// it, which a handler written for [`OpportuneError`] does not catch, so a
/// document argument is read through this helper and an unreadable one is
/// rejected with the code the same document already reports when it arrives with
/// the surrogate ASCII-escaped.
pub(crate) fn document<'a>(text: &'a Bound<'_, PyString>) -> PyResult<&'a str> {
    utf8(
        text,
        "invalid_json",
        "the document is not UTF-8 text, so it has no JSON spelling",
    )
}

/// The text a non-document string argument holds — a zone name, a rule ID, a
/// database directory.
///
/// These are read through the same helper as a document, for the same reason: a
/// lone surrogate in one of them has no UTF-8 spelling this boundary can read,
/// and `PyO3`'s own extraction would raise `UnicodeEncodeError`, which no
/// `OpportuneError` handler catches.
pub(crate) fn argument<'a>(text: &'a Bound<'_, PyString>) -> PyResult<&'a str> {
    utf8(text, ARGUMENT_NOT_UTF8, "the argument is not UTF-8 text")
}

/// `text` as UTF-8, or [`OpportuneError`] carrying `code` when it has no such
/// spelling.
fn utf8<'a>(text: &'a Bound<'_, PyString>, code: &'static str, what: &str) -> PyResult<&'a str> {
    text.to_str().map_err(|cause| {
        rejection(
            text.py(),
            &JsonError::new(code, format!("{what}: {cause}"), ""),
        )
    })
}

/// Runs `work` without holding the GIL and raises its failure as
/// [`OpportuneError`].
///
/// Everything expensive crosses this helper: parsing the document, the Rust
/// evaluation, and writing the answer back out. A Python thread calling into the
/// engine therefore blocks only itself, and a `FastAPI` worker sharing one engine
/// with its siblings sees them run in parallel.
///
/// The release is [`Python::detach`], which suspends this thread's attachment to
/// the interpreter, so the closure may own or borrow only `Send` data and never a
/// `Bound<PyAny>`. A `&str` argument qualifies: Python strings are immutable, and
/// the call's own argument tuple keeps the object alive for the whole call, so
/// another thread can neither free nor change it while this one is detached. A
/// `&self` on a frozen engine qualifies for the same reason — it is shared, not
/// owned here.
pub(crate) fn detach<T, F>(py: Python<'_>, work: F) -> PyResult<T>
where
    F: FnOnce() -> Result<T, JsonError> + Send,
    T: Send,
{
    match py.detach(work) {
        Ok(value) => Ok(value),
        Err(error) => Err(rejection(py, &error)),
    }
}

/// The exception `error` raises.
///
/// [`raise`] reports a refused attribute assignment as that refusal rather than
/// hiding it; either way what comes back is an exception to raise, so the
/// distinction collapses here.
pub(crate) fn rejection(py: Python<'_>, error: &JsonError) -> PyErr {
    raise(py, error).unwrap_or_else(|refusal| refusal)
}

/// The exception `error` raises, with every attribute a handler reads set.
fn raise(py: Python<'_>, error: &JsonError) -> PyResult<PyErr> {
    let raised = OpportuneError::new_err(error.message().to_owned());
    // The attributes are set on the instance rather than held in a Rust type so
    // that `OpportuneError` stays an ordinary Python exception: `str(exc)` is the
    // message, and `except OpportuneError` behaves as it does for any other.
    // Setting them cannot be refused — the instance is this module's own
    // `Exception` subclass, carrying the `__dict__` every such instance has — and
    // a refusal would be reported rather than hidden.
    let value = raised.value(py);
    value.setattr("code", error.code())?;
    value.setattr("message", error.message())?;
    // `""` names the document as a whole, which is Python's `None` here: an empty
    // string is not a pointer a handler could dereference.
    value.setattr("path", Some(error.path()).filter(|path| !path.is_empty()))?;
    value.setattr("errors", findings(py, error)?)?;
    Ok(raised)
}

/// The findings a rejected request carries, as a list of plain dicts.
fn findings<'py>(py: Python<'py>, error: &JsonError) -> PyResult<Bound<'py, PyList>> {
    let findings = PyList::empty(py);
    for finding in error.errors() {
        let entry = PyDict::new(py);
        entry.set_item("code", finding.code())?;
        entry.set_item("message", finding.message())?;
        entry.set_item("path", finding.path())?;
        findings.append(entry)?;
    }
    Ok(findings)
}

#[cfg(test)]
mod tests {
    use super::detach;
    use opportune_json::JsonError;
    use pyo3::prelude::*;
    use pyo3::types::PyDict;

    #[test]
    fn a_rejected_request_raises_an_exception_carrying_its_findings() {
        Python::initialize();
        Python::attach(|py| {
            let findings = vec![
                opportune::Diagnostic::new("zone_unknown", "unknown time zone `Mars/Olympus_Mons`")
                    .with_path("/participants/0/zone"),
            ];
            let error = JsonError::from(opportune::Error::invalid_request(findings));
            let message = error.message().to_owned();
            let raised = detach::<(), _>(py, || Err(error)).expect_err("a failure is raised");

            let value = raised.value(py);
            assert_eq!(
                text(value, "code"),
                "invalid_request",
                "the domain failure's own code is kept"
            );
            assert_eq!(text(value, "message"), message);
            assert_eq!(
                value
                    .getattr("path")
                    .expect("path is set")
                    .extract::<Option<String>>()
                    .expect("a string or None"),
                None,
                "the document as a whole has no pointer, so the empty one is `None`"
            );
            let errors = value
                .getattr("errors")
                .expect("errors is set")
                .extract::<Vec<Bound<'_, PyDict>>>()
                .expect("the findings are a list of dicts");
            assert_eq!(errors.len(), 1);
            let finding = &errors[0];
            assert_eq!(item(finding, "code"), "zone_unknown");
            assert_eq!(
                item(finding, "path"),
                "/participants/0/zone",
                "each finding keeps its own RFC 6901 pointer"
            );
            assert_eq!(
                value.str().expect("the exception formats").to_string(),
                message,
                "`str(exc)` is the message"
            );
        });
    }

    #[test]
    fn a_failure_about_one_field_carries_its_pointer() {
        Python::initialize();
        Python::attach(|py| {
            let error = JsonError::new(
                "instant_not_aligned",
                "not aligned",
                "/goal/search_window/0",
            );
            let raised = detach::<(), _>(py, || Err(error)).expect_err("a failure is raised");

            let path = raised
                .value(py)
                .getattr("path")
                .expect("path is set")
                .extract::<Option<String>>()
                .expect("a string or None");
            assert_eq!(path.as_deref(), Some("/goal/search_window/0"));
        });
    }

    /// `key`'s value in `entry`, as a string.
    fn item(entry: &Bound<'_, PyDict>, key: &str) -> String {
        let value = entry
            .get_item(key)
            .unwrap_or_else(|err| panic!("{key} is readable: {err}"))
            .unwrap_or_else(|| panic!("{key} is present"));
        value
            .extract::<String>()
            .unwrap_or_else(|err| panic!("{key} is a string: {err}"))
    }

    /// `object`'s `attribute`, as a string.
    fn text(object: &Bound<'_, PyAny>, attribute: &str) -> String {
        let value = object
            .getattr(attribute)
            .unwrap_or_else(|err| panic!("{attribute} is set on the exception: {err}"));
        value
            .extract::<String>()
            .unwrap_or_else(|err| panic!("{attribute} is a string: {err}"))
    }
}
