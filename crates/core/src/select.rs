//! Resolve `--me` to one player: account-UUID prefix, pawn id, or agent name.

use anyhow::{Result, bail};

use crate::catalog::Catalog;
use crate::vrfkit_export::Player;

pub fn select_player<'a>(players: &'a [Player], catalog: &Catalog, selector: &str) -> Result<&'a Player> {
    let sel = selector.trim().to_ascii_lowercase();
    if sel.is_empty() {
        bail!("--me is empty");
    }
    let mut matches: Vec<(&Player, &str)> = Vec::new();
    for p in players {
        let how = if sel.parse::<u32>().is_ok_and(|g| p.pawns.contains(&g)) {
            Some("pawn id")
        } else if sel.len() >= 4 && p.subject.starts_with(&sel) {
            Some("account UUID prefix")
        } else if catalog.agent_name(p.agent_id.as_deref()).to_ascii_lowercase() == sel {
            Some("agent name")
        } else {
            None
        };
        if let Some(how) = how {
            matches.push((p, how));
        }
    }
    match matches.as_slice() {
        [(p, _)] => Ok(p),
        [] => bail!(
            "--me `{selector}` matches no player. Use an account-UUID prefix (4+ chars), a pawn id, or an agent name; \
             `vantage players <export_dir>` lists them"
        ),
        many => {
            let list = many
                .iter()
                .map(|(p, how)| {
                    format!(
                        "  pawn {} {} {} (by {how})",
                        p.pawn,
                        catalog.agent_name(p.agent_id.as_deref()),
                        p.subject
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            bail!("--me `{selector}` is ambiguous; it matches:\n{list}\nUse a longer UUID prefix or the pawn id")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(pawn: u32, subject: &str, agent: &str) -> Player {
        Player {
            subject: subject.into(),
            actor_net_guid: 0,
            pawn,
            pawns: vec![pawn],
            agent_id: Some(agent.into()),
        }
    }

    #[test]
    fn selectors() {
        let sova = "320b2a48-4d9b-a075-30f1-1f93a9b638fa";
        let neon = "bb2a4828-46eb-8cd1-e765-15848195d751";
        let c = Catalog::builtin();
        let ps = [
            player(1276, "43082880-1822-5e84-a3f2-337d8a51a958", sova),
            player(554, "b8e94743-9c57-5fee-b861-d255da964e5e", neon),
            player(1376, "d62b0170-f856-527e-ac35-c65b1717116a", neon),
        ];
        assert_eq!(select_player(&ps, &c, "1276").unwrap().pawn, 1276);
        assert_eq!(select_player(&ps, &c, "4308").unwrap().pawn, 1276);
        assert_eq!(select_player(&ps, &c, "SOVA").unwrap().pawn, 1276);
        assert!(select_player(&ps, &c, "neon").is_err());
        assert!(select_player(&ps, &c, "jett").is_err());
        assert!(select_player(&ps, &c, "43").is_err());
    }
}
