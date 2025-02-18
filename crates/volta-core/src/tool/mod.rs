use std::fmt::{self, Display};

use crate::error::{ErrorKind, Fallible};
use crate::session::Session;
use crate::style::{note_prefix, success_prefix, tool_version};
use crate::sync::VoltaLock;
use crate::version::VersionSpec;
use log::{debug, info};

pub mod node;
pub mod npm;
pub mod package;
mod registry;
mod serial;
pub mod yarn;

pub use node::{
    load_default_npm_version, Node, NODE_DISTRO_ARCH, NODE_DISTRO_EXTENSION, NODE_DISTRO_OS,
};
pub use npm::{BundledNpm, Npm};
pub use package::{BinConfig, Package, PackageConfig, PackageManifest};
pub use registry::PackageDetails;
pub use yarn::Yarn;

#[inline]
fn debug_already_fetched<T: Display + Sized>(tool: T) {
    debug!("{} has already been fetched, skipping download", tool);
}

#[inline]
fn info_installed<T: Display + Sized>(tool: T) {
    info!("{} installed and set {} as default", success_prefix(), tool);
}

#[inline]
fn info_fetched<T: Display + Sized>(tool: T) {
    info!("{} fetched {}", success_prefix(), tool);
}

#[inline]
fn info_pinned<T: Display + Sized>(tool: T) {
    info!("{} pinned {} in package.json", success_prefix(), tool);
}

#[inline]
fn info_project_version<T: Display + Sized>(tool: T) {
    info!(
        "{} you are using {} in the current project",
        note_prefix(),
        tool
    );
}

/// Trait representing all of the actions that can be taken with a tool
pub trait Tool: Display {
    /// Fetch a Tool into the local inventory
    fn fetch(self: Box<Self>, session: &mut Session) -> Fallible<()>;
    /// Install a tool, making it the default so it is available everywhere on the user's machine
    fn install(self: Box<Self>, session: &mut Session) -> Fallible<()>;
    /// Pin a tool in the local project so that it is usable within the project
    fn pin(self: Box<Self>, session: &mut Session) -> Fallible<()>;
}

/// Specification for a tool and its associated version.
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub enum Spec {
    Node(VersionSpec),
    Npm(VersionSpec),
    Yarn(VersionSpec),
    Package(String, VersionSpec),
}

impl Spec {
    /// Resolve a tool spec into a fully realized Tool that can be fetched
    pub fn resolve(self, session: &mut Session) -> Fallible<Box<dyn Tool>> {
        match self {
            Spec::Node(version) => {
                let version = node::resolve(version, session)?;
                Ok(Box::new(Node::new(version)))
            }
            Spec::Npm(version) => match npm::resolve(version, session)? {
                Some(version) => Ok(Box::new(Npm::new(version))),
                None => Ok(Box::new(BundledNpm)),
            },
            Spec::Yarn(version) => {
                let version = yarn::resolve(version, session)?;
                Ok(Box::new(Yarn::new(version)))
            }
            // When using global package install, we allow the package manager to perform the version resolution
            Spec::Package(name, version) => {
                let package = Package::new(name, version)?;
                Ok(Box::new(package))
            }
        }
    }

    /// Uninstall a tool, removing it from the local inventory
    ///
    /// This is implemented on Spec, instead of Resolved, because there is currently no need to
    /// resolve the specific version before uninstalling a tool.
    pub fn uninstall(self) -> Fallible<()> {
        match self {
            Spec::Node(_) => Err(ErrorKind::Unimplemented {
                feature: "Uninstalling node".into(),
            }
            .into()),
            Spec::Npm(_) => Err(ErrorKind::Unimplemented {
                feature: "Uninstalling npm".into(),
            }
            .into()),
            Spec::Yarn(_) => Err(ErrorKind::Unimplemented {
                feature: "Uninstalling yarn".into(),
            }
            .into()),
            Spec::Package(name, _) => package::uninstall(&name),
        }
    }

    /// The name of the tool, without the version, used for messaging
    pub fn name(&self) -> &str {
        match self {
            Spec::Node(_) => "Node",
            Spec::Npm(_) => "npm",
            Spec::Yarn(_) => "Yarn",
            Spec::Package(name, _) => name,
        }
    }
}

impl Display for Spec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Spec::Node(ref version) => tool_version("node", version),
            Spec::Npm(ref version) => tool_version("npm", version),
            Spec::Yarn(ref version) => tool_version("yarn", version),
            Spec::Package(ref name, ref version) => tool_version(name, version),
        };
        f.write_str(&s)
    }
}

/// Represents the result of checking if a tool is available locally or not
///
/// If a fetch is required, will include an exclusive lock on the Volta directory where possible
enum FetchStatus {
    AlreadyFetched,
    FetchNeeded(Option<VoltaLock>),
}

/// Uses the supplied `already_fetched` predicate to determine if a tool is available or not.
///
/// This uses double-checking logic, to correctly handle concurrent fetch requests:
///
/// - If `already_fetched` indicates that a fetch is needed, we acquire an exclusive lock on the Volta directory
/// - Then, we check _again_, to confirm that no other process completed the fetch while we waited for the lock
///
/// Note: If acquiring the lock fails, we proceed anyway, since the fetch is still necessary.
fn check_fetched<F>(already_fetched: F) -> Fallible<FetchStatus>
where
    F: Fn() -> Fallible<bool>,
{
    if !already_fetched()? {
        let lock = match VoltaLock::acquire() {
            Ok(l) => Some(l),
            Err(_) => {
                debug!("Unable to acquire lock on Volta directory!");
                None
            }
        };

        if !already_fetched()? {
            Ok(FetchStatus::FetchNeeded(lock))
        } else {
            Ok(FetchStatus::AlreadyFetched)
        }
    } else {
        Ok(FetchStatus::AlreadyFetched)
    }
}

fn download_tool_error(tool: Spec, from_url: impl AsRef<str>) -> impl FnOnce() -> ErrorKind {
    let from_url = from_url.as_ref().to_string();
    || ErrorKind::DownloadToolNetworkError { tool, from_url }
}

fn registry_fetch_error(
    tool: impl AsRef<str>,
    from_url: impl AsRef<str>,
) -> impl FnOnce() -> ErrorKind {
    let tool = tool.as_ref().to_string();
    let from_url = from_url.as_ref().to_string();
    || ErrorKind::RegistryFetchError { tool, from_url }
}
