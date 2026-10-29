use crate::{epic, model::*, steam};

pub fn scan(
    options: &ScanOptions,
    owned: &OwnedLibrary,
    online: &[EpicOnlineLibrary],
    previous: Option<&Report>,
) -> Report {
    let mut report = steam::scan(options, owned, previous);
    let (epic, warnings) = epic::scan(options, &report, online, previous.map(|r| &r.epic));
    report.epic = epic;
    report.warnings.extend(warnings);
    report.warnings.sort();
    report.warnings.dedup();
    report
}
