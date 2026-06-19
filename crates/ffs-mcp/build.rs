//! macOS provisioning-profile embed (task_33 + ADR-023).
//!
//! When `FFS_PROVISIONING_PROFILE` is set at build time AND we're
//! compiling for macOS, embed the file at that path into the
//! binary's `__TEXT,__provisioning` Mach-O section. AMFI reads
//! this section at exec to verify that any restricted entitlements
//! the binary claims (most importantly `keychain-access-groups`)
//! are allowlisted by an Apple-signed profile.
//!
//! Without this embed, an entitlement-carrying Developer-ID-signed
//! binary is silently `SIGKILL`ed by the kernel before main() runs
//! (exit 137, zero stderr — verified empirically). With it, the
//! same binary launches normally and the keychain path is
//! authorized for the team-prefixed access group.
//!
//! Non-macOS builds skip this entirely. Builds without the env var
//! also skip — that's the unsigned dev-loop where the runtime
//! `is_signed_with_keychain_entitlement` check kicks in and the
//! daemon falls back to env-var / generate.

fn main() {
    println!("cargo:rerun-if-env-changed=FFS_PROVISIONING_PROFILE");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let Ok(profile_path) = std::env::var("FFS_PROVISIONING_PROFILE") else {
        return;
    };
    if profile_path.is_empty() {
        return;
    }
    println!("cargo:rerun-if-changed={profile_path}");
    println!("cargo:rustc-link-arg=-Wl,-sectcreate,__TEXT,__provisioning,{profile_path}");
}
