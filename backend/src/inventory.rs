use crate::{adapters, ea, epic, model::*, steam};

pub fn scan(
    options: &ScanOptions,
    owned: &OwnedLibrary,
    online: &[EpicOnlineLibrary],
    previous: Option<&Report>,
) -> Report {
    let mut report = steam::scan(options, owned, previous);
    let prefixes = crate::discovery::prefixes(&report);
    let (epic, warnings) = epic::scan(
        options,
        &report,
        online,
        previous.map(|r| &r.epic),
        &prefixes,
    );
    report.epic = epic;
    report.warnings.extend(warnings);
    let (ea, warnings) = ea::scan(&report, previous.map(|r| &r.ea), &prefixes);
    report.ea = ea;
    report.warnings.extend(warnings);
    let (catalog, warnings) = adapters::scan(&report, previous, prefixes);
    report.catalog = catalog;
    crate::catalog::remove_host_mount_aliases(&mut report.catalog);
    report.linux_systems = crate::linux_support::scan(&report);
    crate::catalog::hydrate_os(&mut report);
    crate::catalog::hydrate_install_actions(&mut report);
    report.scanned_at = crate::util::unix_now();
    report.warnings.extend(warnings);
    report.warnings.sort();
    report.warnings.dedup();
    report
}
