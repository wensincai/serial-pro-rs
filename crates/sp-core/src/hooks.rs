//! Lua 双向数据钩子（mlua，Lua 5.4）。
//!
//! 用户脚本定义全局函数：
//! - `on_send(data)` -> 新数据：发送前调用，返回值替换发送内容
//! - `on_recv(data)` -> 新数据：接收后调用，返回值替换显示内容
//!
//! `data` 是 Lua 字符串（Lua 5.4 字符串 8-bit 干净，可直接承载任意字节）。
//! 返回 `nil` 表示保持原数据不变。
//!
//! 内置 `sp` 工具表：
//! - `sp.hex(s)`       -> "41 42" 十六进制文本
//! - `sp.fromhex("41 42")` -> 原始字节串
//! - `sp.log(msg)` / `print(...)` -> 输出到脚本输出面板（不污染 stdout）

use std::collections::VecDeque;

use mlua::{Lua, Table, Value};

use crate::error::{Error, Result};

/// 默认示例脚本：收发原样透传，同时打印 Hex。
pub const DEFAULT_SCRIPT: &str = r#"-- 双向钩子示例
-- data 为 Lua 字符串（承载原始字节），返回新字符串即完成替换
-- sp.hex(s) / sp.fromhex("AA 55") / sp.log(...) 可用

function on_send(data)
  -- 例：给所有发送自动追加 \r\n
  -- return data .. "\r\n"
  return data
end

function on_recv(data)
  -- 例：把 "t=25.3" 之类文本打到日志
  -- sp.log("RX: " .. sp.hex(data))
  return data
end
"#;

pub struct LuaHooks {
    lua: Lua,
    /// 已编译的脚本（chunk 环境）
    loaded: bool,
    /// 收集 print/sp.log 输出（环形）
    prints: VecDeque<String>,
}

impl LuaHooks {
    pub fn new() -> Result<Self> {
        let lua = Lua::new();
        let this = Self {
            lua,
            loaded: false,
            prints: VecDeque::new(),
        };
        Ok(this)
    }

    /// （重）加载脚本。加载失败保留旧脚本可用性（loaded 不翻转）。
    pub fn load_script(&mut self, script: &str) -> Result<()> {
        // 重置全局环境
        self.lua.globals().set("on_send", Value::Nil)?;
        self.lua.globals().set("on_recv", Value::Nil)?;
        self.prints.clear();
        self.lua
            .set_named_registry_value("sp_prints", Vec::<String>::new())?;

        let chunk = self.lua.load(script).set_name("sp_hook");
        chunk
            .exec()
            .map_err(|e| Error::Hook(format!("脚本执行失败: {e}")))?;

        // 安装 sp 工具表
        let lua = &self.lua;
        let sp: Table = lua.create_table()?;

        let hex_fn = lua.create_function(|_, s: mlua::String| {
            let bytes = s.as_bytes().to_vec();
            Ok(crate::hexutil::to_hex(&bytes, " "))
        })?;
        sp.set("hex", hex_fn)?;

        let fromhex_fn = lua.create_function(|lua, s: String| {
            let bytes = crate::hexutil::parse_hex(&s).map_err(mlua::Error::external)?;
            lua.create_string(bytes)
        })?;
        sp.set("fromhex", fromhex_fn)?;

        self.lua.globals().set("sp", sp)?;

        // print -> 注册表暂存，transform 侧统一取出
        let print_fn = lua.create_function(|lua, args: mlua::MultiValue| {
            let mut line = String::new();
            for (i, v) in args.into_iter().enumerate() {
                if i > 0 {
                    line.push('\t');
                }
                match v {
                    Value::String(s) => {
                        line.push_str(&String::from_utf8_lossy(&s.as_bytes()[..]))
                    }
                    other => line.push_str(&format!("{other:?}")),
                }
            }
            let mut buf: Vec<String> = lua.named_registry_value("sp_prints")?;
            buf.push(line);
            lua.set_named_registry_value("sp_prints", buf)?;
            Ok(())
        })?;
        self.lua.globals().set("print", print_fn)?;

        self.loaded = true;
        Ok(())
    }

    pub fn loaded(&self) -> bool {
        self.loaded
    }

    /// 取出脚本输出（print/sp.log 收集）。
    pub fn take_prints(&mut self) -> Vec<String> {
        let mut own = self
            .lua
            .named_registry_value::<Vec<String>>("sp_prints")
            .unwrap_or_default();
        self.lua
            .set_named_registry_value("sp_prints", Vec::<String>::new())
            .ok();
        own.append(&mut self.prints.drain(..).collect::<Vec<_>>());
        own
    }

    /// 发送前钩子。返回 (可能被替换的数据, 错误信息)。
    pub fn transform_send(&mut self, data: &[u8]) -> (Vec<u8>, Option<String>) {
        self.transform("on_send", data)
    }

    /// 接收后钩子。
    pub fn transform_recv(&mut self, data: &[u8]) -> (Vec<u8>, Option<String>) {
        self.transform("on_recv", data)
    }

    fn transform(&mut self, func: &str, data: &[u8]) -> (Vec<u8>, Option<String>) {
        if !self.loaded {
            return (data.to_vec(), None);
        }
        let result = (|| -> mlua::Result<Option<Vec<u8>>> {
            let f = self.lua.globals().get::<Value>(func)?;
            let Value::Function(f) = f else {
                return Ok(None); // 未定义该函数：透传
            };
            let arg = self.lua.create_string(data)?;
            let ret: Value = f.call(arg)?;
            match ret {
                Value::Nil => Ok(None),
                Value::String(s) => Ok(Some(s.as_bytes().to_vec())),
                other => Ok(Some(format!("{other:?}").into_bytes())),
            }
        })();
        match result {
            Ok(Some(modified)) => (modified, None),
            Ok(None) => (data.to_vec(), None),
            Err(e) => (data.to_vec(), Some(format!("{func}: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(script: &str) -> LuaHooks {
        let mut h = LuaHooks::new().unwrap();
        h.load_script(script).unwrap();
        h
    }

    #[test]
    fn passthrough_without_functions() {
        let mut h = engine("-- no functions");
        let (out, err) = h.transform_send(b"abc");
        assert_eq!(out, b"abc");
        assert!(err.is_none());
    }

    #[test]
    fn send_append_crlf() {
        let mut h = engine(
            r#"
            function on_send(data)
              return data .. "\r\n"
            end
        "#,
        );
        let (out, err) = h.transform_send(b"AT");
        assert_eq!(out, b"AT\r\n");
        assert!(err.is_none());
    }

    #[test]
    fn recv_binary_safe() {
        let mut h = engine(
            r#"
            function on_recv(data)
              return sp.fromhex("AA 55")
            end
        "#,
        );
        let (out, err) = h.transform_recv(&[1, 2, 3]);
        assert_eq!(out, vec![0xAA, 0x55]);
        assert!(err.is_none());
    }

    #[test]
    fn nil_keeps_original() {
        let mut h = engine(
            r#"
            function on_send(data) return nil end
        "#,
        );
        let (out, _) = h.transform_send(b"keep");
        assert_eq!(out, b"keep");
    }

    #[test]
    fn error_reports_and_keeps_original() {
        let mut h = engine(
            r#"
            function on_recv(data)
              error("boom")
            end
        "#,
        );
        let (out, err) = h.transform_recv(b"x");
        assert_eq!(out, b"x");
        let err = err.unwrap();
        assert!(err.contains("boom"), "err={err}");
    }

    #[test]
    fn print_collected() {
        let mut h = engine(
            r#"
            function on_recv(data)
              print("hello", 42)
              return data
            end
        "#,
        );
        let _ = h.transform_recv(b"x");
        let prints = h.take_prints();
        assert_eq!(prints.len(), 1);
        assert!(prints[0].contains("hello"));
        assert!(prints[0].contains("42"));
    }

    #[test]
    fn reload_resets() {
        let mut h = engine(
            r#"
            function on_send(data) return "v1" end
        "#,
        );
        let (out, _) = h.transform_send(b"");
        assert_eq!(out, b"v1");
        // 重载为别的脚本
        h.load_script("function on_send(data) return 'v2' end").unwrap();
        let (out, _) = h.transform_send(b"");
        assert_eq!(out, b"v2");
    }

    #[test]
    fn syntax_error_keeps_old() {
        let mut h = engine("function on_send(d) return d end");
        assert!(h.load_script("this is not lua !!!").is_err());
        // 旧脚本仍然可用
        let (out, _) = h.transform_send(b"");
        assert_eq!(out, b"");
    }
}
