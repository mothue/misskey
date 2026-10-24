// Generated from packages/backend/src/core/RoleService.ts. Do not edit.
// Regenerate: bun packages/backend-rust/scripts/generate-policy-aggregation.ts

#[derive(Clone, Copy)]
pub(super) enum Aggregation {
    Any,
    Max,
    MaxFileSize,
    Chat,
    FileTypes,
}

pub(super) fn policy_aggregation(name: &str) -> Option<Aggregation> {
    Some(match name {
        "gtlAvailable" => Aggregation::Any,
        "ltlAvailable" => Aggregation::Any,
        "canPublicNote" => Aggregation::Any,
        "mentionLimit" => Aggregation::Max,
        "canInvite" => Aggregation::Any,
        "inviteLimit" => Aggregation::Max,
        "inviteLimitCycle" => Aggregation::Max,
        "inviteExpirationTime" => Aggregation::Max,
        "canManageCustomEmojis" => Aggregation::Any,
        "canManageAvatarDecorations" => Aggregation::Any,
        "canSearchNotes" => Aggregation::Any,
        "canSearchUsers" => Aggregation::Any,
        "canUseTranslator" => Aggregation::Any,
        "canHideAds" => Aggregation::Any,
        "canCreateChannel" => Aggregation::Any,
        "driveCapacityMb" => Aggregation::Max,
        "maxFileSizeMb" => Aggregation::MaxFileSize,
        "alwaysMarkNsfw" => Aggregation::Any,
        "canUpdateBioMedia" => Aggregation::Any,
        "pinLimit" => Aggregation::Max,
        "antennaLimit" => Aggregation::Max,
        "wordMuteLimit" => Aggregation::Max,
        "webhookLimit" => Aggregation::Max,
        "clipLimit" => Aggregation::Max,
        "noteEachClipsLimit" => Aggregation::Max,
        "userListLimit" => Aggregation::Max,
        "userEachUserListsLimit" => Aggregation::Max,
        "rateLimitFactor" => Aggregation::Max,
        "avatarDecorationLimit" => Aggregation::Max,
        "canImportAntennas" => Aggregation::Any,
        "canImportBlocking" => Aggregation::Any,
        "canImportFollowing" => Aggregation::Any,
        "canImportMuting" => Aggregation::Any,
        "canImportUserLists" => Aggregation::Any,
        "chatAvailability" => Aggregation::Chat,
        "uploadableFileTypes" => Aggregation::FileTypes,
        "noteDraftLimit" => Aggregation::Max,
        "scheduledNoteLimit" => Aggregation::Max,
        "watermarkAvailable" => Aggregation::Any,
        _ => return None,
    })
}
