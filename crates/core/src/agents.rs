//! Agent names. A built-in table (checked against valorant-api.com
//! `/v1/agents` on 2026-10-20) keeps the CLI usable offline; the Phase 2
//! catalog will take precedence over it.

const BUILTIN: &[(&str, &str, &str)] = &[
    ("41fb69c1-4189-7b37-f117-bcaf1e96f1bf", "Astra", "Controller"),
    ("5f8d3a7f-467b-97f3-062c-13acf203c006", "Breach", "Initiator"),
    ("9f0d8ba9-4140-b941-57d3-a7ad57c6b417", "Brimstone", "Controller"),
    ("22697a3d-45bf-8dd7-4fec-84a9e28c69d7", "Chamber", "Sentinel"),
    ("1dbf2edd-4729-0984-3115-daa5eed44993", "Clove", "Controller"),
    ("117ed9e3-49f3-6512-3ccf-0cada7e3823b", "Cypher", "Sentinel"),
    ("cc8b64c8-4b25-4ff9-6e7f-37b4da43d235", "Deadlock", "Sentinel"),
    ("dade69b4-4f5a-8528-247b-219e5a1facd6", "Fade", "Initiator"),
    ("e370fa57-4757-3604-3648-499e1f642d3f", "Gekko", "Initiator"),
    ("95b78ed7-4637-86d9-7e41-71ba8c293152", "Harbor", "Controller"),
    ("0e38b510-41a8-5780-5e8f-568b2a4f2d6c", "Iso", "Duelist"),
    ("add6443a-41bd-e414-f6ad-e58d267f4e95", "Jett", "Duelist"),
    ("601dbbe7-43ce-be57-2a40-4abd24953621", "KAY/O", "Initiator"),
    ("1e58de9c-4950-5125-93e9-a0aee9f98746", "Killjoy", "Sentinel"),
    ("7c8a4701-4de6-9355-b254-e09bc2a34b72", "Miks", "Controller"),
    ("bb2a4828-46eb-8cd1-e765-15848195d751", "Neon", "Duelist"),
    ("8e253930-4c05-31dd-1b6c-968525494517", "Omen", "Controller"),
    ("eb93336a-449b-9c1b-0a54-a891f7921d69", "Phoenix", "Duelist"),
    ("f94c3b30-42be-e959-889c-5aa313dba261", "Raze", "Duelist"),
    ("a3bfb853-43b2-7238-a4f1-ad90e9e46bcc", "Reyna", "Duelist"),
    ("569fdd95-4d10-43ab-ca70-79becc718b46", "Sage", "Sentinel"),
    ("6f2a04ca-43e0-be17-7f36-b3908627744d", "Skye", "Initiator"),
    ("320b2a48-4d9b-a075-30f1-1f93a9b638fa", "Sova", "Initiator"),
    ("b444168c-4e35-8076-db47-ef9bf368f384", "Tejo", "Initiator"),
    ("92eeef5d-43b5-1d4a-8d03-b3927a09034b", "Veto", "Sentinel"),
    ("707eab51-4836-f488-046a-cda6bf494859", "Viper", "Controller"),
    ("efba5359-4016-a1e5-7626-b1ae76895940", "Vyse", "Sentinel"),
    ("df1cb487-4902-002e-5c17-d28e83e78588", "Waylay", "Duelist"),
    ("7f94d92c-4234-0a36-9646-3a87eb8b5c89", "Yoru", "Duelist"),
];

/// Display name for an agent UUID, or `agent_<first 8 hex>` when unknown.
pub fn agent_name(agent_id: Option<&str>) -> String {
    let Some(id) = agent_id else {
        return "unknown".into();
    };
    let id = id.to_ascii_lowercase();
    BUILTIN
        .iter()
        .find(|(uuid, _, _)| *uuid == id)
        .map(|(_, name, _)| (*name).to_owned())
        .unwrap_or_else(|| {
            format!(
                "agent_{}",
                id.chars().filter(|c| c.is_ascii_hexdigit()).take(8).collect::<String>()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_and_unknown() {
        assert_eq!(agent_name(Some("320B2A48-4d9b-a075-30f1-1f93a9b638fa")), "Sova");
        assert_eq!(
            agent_name(Some("deadbeef-0000-0000-0000-000000000000")),
            "agent_deadbeef"
        );
        assert_eq!(agent_name(None), "unknown");
    }
}
