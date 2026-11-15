use super::*;

pub(super) fn config(feed: &mut Feed, command: &Config) -> Result<(), String> {
    let snapshot = feed
        .settings
        .values_mut()
        .next()
        .ok_or("Preview settings not found")?;
    let expected = match command {
        Config::Set { expected_hash, .. }
        | Config::ProfileAdd { expected_hash, .. }
        | Config::ProfileRemove { expected_hash, .. } => expected_hash,
        _ => return unsupported(command.verb()),
    };
    if expected != &snapshot.config_hash {
        return Err("conflict: preview configuration changed".into());
    }
    match command {
        Config::Set { key, value, .. } => match key.as_str() {
            "assistant.profile" => {
                if !snapshot
                    .assistant_models
                    .iter()
                    .any(|model| &model.name == value)
                {
                    return Err("Preview profile not found".into());
                }
                snapshot.assistant_profile = Some(value.clone());
            }
            key if key.starts_with("profiles.") && key.ends_with(".model") => {
                let name = &key["profiles.".len()..key.len() - ".model".len()];
                let profile = snapshot
                    .profiles
                    .iter_mut()
                    .find(|profile| profile.name == name)
                    .ok_or("Preview profile not found")?;
                profile.model = Some(value.clone());
                if let Some(choice) = snapshot
                    .assistant_models
                    .iter_mut()
                    .find(|choice| choice.name == name)
                {
                    choice.model = value.clone();
                }
            }
            _ => return unsupported(key),
        },
        Config::ProfileAdd {
            name,
            from,
            model,
            effort,
            ..
        } => {
            if snapshot
                .profiles
                .iter()
                .any(|profile| &profile.name == name)
            {
                return Err("Preview profile already exists".into());
            }
            let mut profile = snapshot
                .profiles
                .iter()
                .find(|profile| &profile.name == from)
                .cloned()
                .ok_or("Preview source profile not found")?;
            profile.name = name.clone();
            profile.model = model.clone().or(profile.model);
            profile.effort = effort.clone().or(profile.effort);
            profile.references.clear();
            if let Some(model) = &profile.model {
                snapshot.assistant_models.push(ModelChoice {
                    name: name.clone(),
                    model: model.clone(),
                    effort: profile.effort.clone(),
                    note: Some("Preview".into()),
                });
            }
            snapshot.profiles.push(profile);
        }
        Config::ProfileRemove { name, .. } => {
            let profile = snapshot
                .profiles
                .iter()
                .find(|profile| &profile.name == name)
                .ok_or("Preview profile not found")?;
            if profile.remove_refusal.is_some() || snapshot.assistant_profile.as_ref() == Some(name)
            {
                return Err("Preview profile is still referenced".into());
            }
            snapshot.profiles.retain(|profile| &profile.name != name);
            snapshot
                .assistant_models
                .retain(|model| &model.name != name);
        }
        _ => return unsupported(command.verb()),
    }
    snapshot.rev += 1;
    snapshot.config_hash = format!("preview-config-{}", snapshot.rev);
    Ok(())
}
