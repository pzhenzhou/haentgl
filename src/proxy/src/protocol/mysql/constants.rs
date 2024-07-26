use num_derive::{FromPrimitive, ToPrimitive};
use std::collections::HashMap;
use strum_macros::AsRefStr;

// see: https://dev.mysql.com/doc/refman/8.0/en/identifier-length.html
// max packet payload length.
pub const MAX_PAYLOAD_LEN: usize = 16_777_215;

pub const ERR_TEXT_LEN: usize = 80;

pub const MAX_KEY_PARTS: usize = 16;

pub const MAX_ALIAS_IDENTIFIER_LEN: usize = 256;

pub const PACKET_HEADER_LEN: usize = 4;
/// auth-plugin-data-part-1 The first 8 bits of a random number will be used for subsequent password encryption.
/// 1 byte padding. 2-byte integer.
pub const AUTH_PLUGIN_DATA_PART_1_LENGTH: usize = 8;
/// The length of the random number required for encryption. (auth-plugin-data-part-1 + auth-plugin-data-part-2)
pub const SCRAMBLE_SIZE: usize = 20;

#[derive(Debug, PartialEq, AsRefStr)]
pub enum AuthPluginName {
    #[strum(serialize = "mysql_old_password")]
    AuthMySQlOldPassword,
    #[strum(serialize = "caching_sha2_password")]
    AuthCachingSha2Password,
    #[strum(serialize = "sha256_password")]
    AuthSha256Password,
    #[strum(serialize = "mysql_native_password")]
    AuthNativePassword,
    #[strum(serialize = "auth_unknown_plugin")]
    UnKnowPluginName,
}

#[derive(Debug, PartialEq, ToPrimitive, FromPrimitive)]
#[repr(u8)]
pub enum HeaderInfo {
    OKHeader = 0x00,
    ErrHeader = 0xff,
    EOFHeader = 0xfe,
    LocalInFileHeader = 0xfb,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, FromPrimitive, ToPrimitive)]
#[repr(u8)]
pub enum CommandCode {
    ComSleep = 0,
    ComQuit,
    ComInitDB,
    ComQuery,
    ComFieldList,
    ComCreateDB,
    ComDropDB,
    ComRefresh,
    ComShutdown,
    ComStatistics,
    ComProcessInfo,
    ComConnect,
    ComProcessKill,
    ComDebug,
    ComPing,
    ComTime,
    ComDelayedInsert,
    ComChangeUser,
    ComBinlogDump,
    ComTableDump,
    ComConnectOut,
    ComRegisterSlave,
    ComStmtPrepare,
    ComStmtExecute,
    ComStmtSendLongData,
    ComStmtClose,
    ComStmtReset,
    ComSetOption,
    ComStmtFetch,
    ComDaemon,
    ComBinlogDumpGtid,
    ComResetConnection,
    ComEnd,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum SqlComInfo {
    ComSleep(u8, &'static str),
    ComQuit(u8, &'static str),
    ComInitDB(u8, &'static str),
    ComQuery(u8, &'static str),
    ComFieldList(u8, &'static str),
    ComCreateDB(u8, &'static str),
    ComDropDB(u8, &'static str),
    ComRefresh(u8, &'static str),
    ComShutdown(u8, &'static str),
    ComStatistics(u8, &'static str),
    ComProcessInfo(u8, &'static str),
    ComConnect(u8, &'static str),
    ComProcessKill(u8, &'static str),
    ComDebug(u8, &'static str),
    ComPing(u8, &'static str),
    ComTime(u8, &'static str),
    ComDelayedInsert(u8, &'static str),
    ComChangeUser(u8, &'static str),
    ComBinlogDump(u8, &'static str),
    ComTableDump(u8, &'static str),
    ComConnectOut(u8, &'static str),
    ComRegisterSlave(u8, &'static str),
    ComStmtPrepare(u8, &'static str),
    ComStmtExecute(u8, &'static str),
    ComStmtSendLongData(u8, &'static str),
    ComStmtClose(u8, &'static str),
    ComStmtReset(u8, &'static str),
    ComSetOption(u8, &'static str),
    ComStmtFetch(u8, &'static str),
    ComDaemon(u8, &'static str),
    ComBinlogDumpGtid(u8, &'static str),
    ComResetConnection(u8, &'static str),
    ComEnd(u8, &'static str),
}

impl SqlComInfo {
    #[inline]
    pub fn all_sql_com() -> &'static HashMap<u8, &'static str> {
        static SQL_COM: std::sync::OnceLock<HashMap<u8, &'static str>> = std::sync::OnceLock::new();
        SQL_COM.get_or_init(|| {
            HashMap::from([
                (0_u8, "ComSleep"),
                (1_u8, "ComQuit"),
                (2_u8, "ComInitDB"),
                (3_u8, "ComQuery"),
                (4_u8, "ComFieldList"),
                (5_u8, "ComCreateDB"),
                (6_u8, "ComDropDB"),
                (7_u8, "ComRefresh"),
                (8_u8, "ComShutdown"),
                (9_u8, "ComStatistics"),
                (10_u8, "ComProcessInfo"),
                (11_u8, "ComConnect"),
                (12_u8, "ComProcessKill"),
                (13_u8, "ComDebug"),
                (14_u8, "ComPing"),
                (15_u8, "ComTime"),
                (16_u8, "ComDelayedInsert"),
                (17_u8, "ComChangeUser"),
                (18_u8, "ComBinlogDump"),
                (19_u8, "ComTableDump"),
                (20_u8, "ComConnectOut"),
                (21_u8, "ComRegisterSlave"),
                (22_u8, "ComStmtPrepare"),
                (23_u8, "ComStmtExecute"),
                (24_u8, "ComStmtSendLongData"),
                (25_u8, "ComStmtClose"),
                (26_u8, "ComStmtReset"),
                (27_u8, "ComSetOption"),
                (28_u8, "ComStmtFetch"),
                (29_u8, "ComDaemon"),
                (30_u8, "ComBinlogDumpGtid"),
                (31_u8, "ComResetConnection"),
                (32_u8, "ComEnd"),
            ])
        })
    }

    pub fn com_as_ref(&self) -> &'static str {
        match self {
            SqlComInfo::ComSleep(_, com_sleep) => com_sleep,
            SqlComInfo::ComQuit(_, quit) => quit,
            SqlComInfo::ComInitDB(_, init_db) => init_db,
            SqlComInfo::ComQuery(_, query) => query,
            SqlComInfo::ComFieldList(_, field_list) => field_list,
            SqlComInfo::ComCreateDB(_, create_db) => create_db,
            SqlComInfo::ComDropDB(_, com_drop_db) => com_drop_db,
            SqlComInfo::ComRefresh(_, com_refresh) => com_refresh,
            SqlComInfo::ComShutdown(_, shut_down) => shut_down,
            SqlComInfo::ComStatistics(_, statistics) => statistics,
            SqlComInfo::ComProcessInfo(_, process_info) => process_info,
            SqlComInfo::ComConnect(_, connect) => connect,
            SqlComInfo::ComProcessKill(_, process_kill) => process_kill,
            SqlComInfo::ComDebug(_, com_debug) => com_debug,
            SqlComInfo::ComPing(_, com_ping) => com_ping,
            SqlComInfo::ComTime(_, com_time) => com_time,
            SqlComInfo::ComDelayedInsert(_, delayed_insert) => delayed_insert,
            SqlComInfo::ComChangeUser(_, change_user) => change_user,
            SqlComInfo::ComBinlogDump(_, binlog_dump) => binlog_dump,
            SqlComInfo::ComTableDump(_, table_dump) => table_dump,
            SqlComInfo::ComConnectOut(_, connect_out) => connect_out,
            SqlComInfo::ComRegisterSlave(_, register_slave) => register_slave,
            SqlComInfo::ComStmtPrepare(_, stmt_prepare) => stmt_prepare,
            SqlComInfo::ComStmtExecute(_, stmt_execute) => stmt_execute,
            SqlComInfo::ComStmtSendLongData(_, stmt_send_long_data) => stmt_send_long_data,
            SqlComInfo::ComStmtClose(_, stmt_close) => stmt_close,
            SqlComInfo::ComStmtReset(_, stmt_reset) => stmt_reset,
            SqlComInfo::ComSetOption(_, set_opts) => set_opts,
            SqlComInfo::ComStmtFetch(_, com_stmt_fetch) => com_stmt_fetch,
            SqlComInfo::ComDaemon(_, daemon) => daemon,
            SqlComInfo::ComBinlogDumpGtid(_, bin_log_dump_gtid) => bin_log_dump_gtid,
            SqlComInfo::ComResetConnection(_, rest_conn) => rest_conn,
            SqlComInfo::ComEnd(_, com_end) => com_end,
        }
    }
}

#[cfg(test)]
mod test {
    use crate::protocol::mysql::constants::*;
    #[allow(unused_imports)]
    use bitflags::Flags;

    #[test]
    pub fn max_packet_size_test() {
        let max_u24_size = 16_777_215;
        assert_eq!(max_u24_size, MAX_PAYLOAD_LEN);
    }

    #[test]
    pub fn column_flag_test() {
        let enum_flag = mysql_common::constants::ColumnFlags::NOT_NULL_FLAG.bits();
        assert_eq!(1_u16, enum_flag);
    }

    #[test]
    pub fn test_common_info_code() {
        let com_info = CommandCode::ComQuery as u8;
        println!("ComQueryCode = {com_info}");
    }
}
