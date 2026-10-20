//! What an equippable or gear actor is: name, kind, credit cost.
//!
//! Matching is by asset folder, which the replay's class paths and
//! valorant-api.com's `assetPath` share (`/Game/Equippables/Guns/Rifles/AK/...`
//! is `ShooterGame/Content/Equippables/Guns/Rifles/AK/AKPrimaryAsset`). The
//! built-in table (valorant-api.com, 2026-10-20) is used when the catalog has
//! no match, so names and costs work offline.

use serde::Serialize;

use crate::catalog::Catalog;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Gun,
    Melee,
    Spike,
    Armor,
    Ability,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Item {
    pub name: String,
    pub kind: ItemKind,
    /// `sidearm`, `smg`, `shotgun`, `rifle`, `sniper`, `heavy` for guns.
    pub category: Option<String>,
    pub cost: Option<u32>,
    pub magazine: Option<u32>,
}

impl Item {
    pub fn is_gun(&self) -> bool {
        self.kind == ItemKind::Gun
    }
}

/// (folder under /Game, name, category, cost, magazine)
const BUILTIN_GUNS: &[(&str, &str, &str, u32, u32)] = &[
    ("Equippables/Guns/HvyMachineGuns/HMG", "Odin", "heavy", 3200, 100),
    ("Equippables/Guns/HvyMachineGuns/LMG", "Ares", "heavy", 1600, 50),
    ("Equippables/Guns/Rifles/AK", "Vandal", "rifle", 2900, 25),
    ("Equippables/Guns/Rifles/BattleRifle", "Warden", "rifle", 2900, 18),
    ("Equippables/Guns/Rifles/Burst", "Bulldog", "rifle", 2050, 24),
    ("Equippables/Guns/Rifles/Carbine", "Phantom", "rifle", 2900, 30),
    ("Equippables/Guns/Shotguns/AutoShotgun", "Judge", "shotgun", 1850, 5),
    ("Equippables/Guns/Shotguns/PumpShotgun", "Bucky", "shotgun", 850, 5),
    ("Equippables/Guns/Sidearms/AutoPistol", "Frenzy", "sidearm", 450, 15),
    ("Equippables/Guns/Sidearms/BasePistol", "Classic", "sidearm", 0, 12),
    ("Equippables/Guns/Sidearms/Compact", "Bandit", "sidearm", 600, 8),
    ("Equippables/Guns/Sidearms/Luger", "Ghost", "sidearm", 500, 13),
    ("Equippables/Guns/Sidearms/Revolver", "Sheriff", "sidearm", 800, 6),
    ("Equippables/Guns/Sidearms/Slim", "Shorty", "sidearm", 300, 2),
    (
        "Equippables/Guns/SniperRifles/Boltsniper",
        "Operator",
        "sniper",
        4700,
        5,
    ),
    ("Equippables/Guns/SniperRifles/DMR", "Guardian", "rifle", 2250, 12),
    (
        "Equippables/Guns/SniperRifles/Doublesniper",
        "Outlaw",
        "sniper",
        2400,
        2,
    ),
    ("Equippables/Guns/SniperRifles/Leversniper", "Marshal", "sniper", 950, 5),
    ("Equippables/Guns/SubMachineGuns/MP5", "Spectre", "smg", 1600, 30),
    ("Equippables/Guns/SubMachineGuns/Vector", "Stinger", "smg", 1100, 20),
];

/// (class leaf prefix, name, cost)
const BUILTIN_ARMOR: &[(&str, &str, u32)] = &[
    ("HeavyArmor", "Heavy Armor", 1000),
    ("LightArmor", "Light Armor", 400),
    ("PlasmaArmor", "Regen Shield", 650),
];

/// `/Game/Equippables/Guns/Rifles/AK/AssaultRifle_AK.AssaultRifle_AK_C` -> `Equippables/Guns/Rifles/AK`.
fn folder(class_path: &str) -> &str {
    let path = class_path.strip_prefix("/Game/").unwrap_or(class_path);
    path.rsplit_once('/').map(|(f, _)| f).unwrap_or("")
}

fn leaf(class_path: &str) -> &str {
    class_path.rsplit('/').next().unwrap_or(class_path)
}

fn api_category(c: &str) -> String {
    match c.trim_start_matches("EEquippableCategory::") {
        "Sidearm" => "sidearm",
        "SMG" => "smg",
        "Shotgun" => "shotgun",
        "Rifle" => "rifle",
        "Sniper" => "sniper",
        "Heavy" => "heavy",
        "Melee" => "melee",
        other => return other.to_ascii_lowercase(),
    }
    .to_owned()
}

pub fn identify(class_path: &str, catalog: &Catalog) -> Item {
    let dir = folder(class_path);
    let leaf = leaf(class_path);
    if class_path.contains("BombEquippable") {
        return Item {
            name: "Spike".into(),
            kind: ItemKind::Spike,
            category: None,
            cost: None,
            magazine: None,
        };
    }
    if dir.starts_with("Equippables/Melee") {
        return Item {
            name: "Melee".into(),
            kind: ItemKind::Melee,
            category: Some("melee".into()),
            cost: None,
            magazine: None,
        };
    }
    if dir.starts_with("Gear") {
        if let Some(g) = catalog
            .gear
            .iter()
            .find(|g| g.class_prefix.as_deref().is_some_and(|p| leaf.starts_with(p)))
        {
            return Item {
                name: g.name.clone(),
                kind: ItemKind::Armor,
                category: None,
                cost: g.cost,
                magazine: None,
            };
        }
        if let Some((_, name, cost)) = BUILTIN_ARMOR.iter().find(|(p, _, _)| leaf.starts_with(p)) {
            return Item {
                name: (*name).into(),
                kind: ItemKind::Armor,
                category: None,
                cost: Some(*cost),
                magazine: None,
            };
        }
    }
    if dir.starts_with("Equippables/Guns") {
        if let Some(w) = catalog
            .weapons
            .iter()
            .find(|w| w.asset_folder.as_deref().is_some_and(|f| f.eq_ignore_ascii_case(dir)))
        {
            return Item {
                name: w.name.clone(),
                kind: ItemKind::Gun,
                category: Some(api_category(&w.category)),
                cost: w.cost,
                magazine: w.magazine_size,
            };
        }
        if let Some((_, name, cat, cost, mag)) = BUILTIN_GUNS.iter().find(|(f, ..)| f.eq_ignore_ascii_case(dir)) {
            return Item {
                name: (*name).into(),
                kind: ItemKind::Gun,
                category: Some((*cat).into()),
                cost: Some(*cost),
                magazine: Some(*mag),
            };
        }
        let name = leaf.split('.').next().unwrap_or(leaf).to_owned();
        return Item {
            name,
            kind: ItemKind::Gun,
            category: None,
            cost: None,
            magazine: None,
        };
    }
    let name = leaf.split('.').next().unwrap_or(leaf).to_owned();
    // Ability items and ability guns (Chamber's Headhunter, Neon's ult) live under /Game/Characters.
    let kind = if dir.starts_with("Characters") {
        ItemKind::Ability
    } else {
        ItemKind::Other
    };
    Item {
        name,
        kind,
        category: None,
        cost: None,
        magazine: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin() {
        let c = Catalog::builtin();
        let v = identify("/Game/Equippables/Guns/Rifles/AK/AssaultRifle_AK.AssaultRifle_AK_C", &c);
        assert_eq!(
            (v.name.as_str(), v.kind, v.cost, v.magazine),
            ("Vandal", ItemKind::Gun, Some(2900), Some(25))
        );
        let g = identify("/Game/Equippables/Guns/SniperRifles/Dmr/DMR.DMR_C", &c);
        assert_eq!(g.name, "Guardian");
        let a = identify("/Game/Gear/HeavyArmorItem.HeavyArmorItem_C", &c);
        assert_eq!((a.name.as_str(), a.cost), ("Heavy Armor", Some(1000)));
        let r = identify("/Game/Gear/PlasmaArmor/PlasmaArmorItem.PlasmaArmorItem_C", &c);
        assert_eq!(r.name, "Regen Shield");
        assert_eq!(
            identify("/Game/Equippables/Melee/Ability_Melee_Base.Ability_Melee_Base_C", &c).kind,
            ItemKind::Melee
        );
        assert_eq!(
            identify("/Game/Equippables/Bomb/BombEquippable.BombEquippable_C", &c).kind,
            ItemKind::Spike
        );
        let h = identify(
            "/Game/Characters/Deadeye/S0/Ability_Q/Gun/Gun_Deadeye_Q_Pistol.Gun_Deadeye_Q_Pistol_C",
            &c,
        );
        assert_eq!(h.kind, ItemKind::Ability);
    }
}
