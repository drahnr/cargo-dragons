use cargo::{
	core::package::Package,
	ops::{modify_owners, OwnersOptions},
	GlobalContext,
};
use cargo_credential::Secret;

pub fn add_owner(
	gctx: &GlobalContext,
	package: &Package,
	new_owner: String,
	token: Option<Secret<String>>,
) -> Result<(), anyhow::Error> {
	if let Err(e) = modify_owners(
		gctx,
		&OwnersOptions {
			token,
			krate: Some(package.name().to_string()),
			to_add: Some(vec![new_owner.clone()]),
			to_remove: None,
			list: false,
			reg_or_index: None,
		},
	) {
		let msg = e.to_string();
		if !msg.ends_with("is already an owner") {
			anyhow::bail!(msg)
		}

		gctx.shell()
			.status("Owner", format!("{:} is already an owner of {:}", new_owner, package.name()))
			.expect("Shell worked before. qed")
	}

	Ok(())
}
