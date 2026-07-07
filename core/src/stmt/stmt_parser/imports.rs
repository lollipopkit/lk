use super::StmtParser;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::{
    stmt::{ImportItem, ImportSource, ImportStmt, Stmt},
    token::Token,
};
use anyhow::{Result, anyhow};

impl<'a> StmtParser<'a> {
    pub fn parse_import_stmt(&mut self) -> Result<Stmt> {
        self.expect_token(Token::Use)?;

        // After 'use', ensure there is a specifier
        if self.eof() {
            return Err(anyhow!(self.err("Expected use specifier after 'use'")));
        }

        // Check for different use patterns
        let import_stmt = match &self.tokens[self.pos] {
            // use "path";
            Token::Str(path) => {
                let path = path.clone();
                self.pos += 1;
                ImportStmt::File { path }
            }
            // use { ... } from source
            Token::LBrace => {
                self.pos += 1; // consume {
                let items = self.parse_import_items()?;
                self.expect_token(Token::RBrace)?;
                self.expect_token(Token::From)?;
                let source = self.parse_import_source()?;
                ImportStmt::Items { items, source }
            }
            // use * as alias from source
            Token::Mul => {
                self.pos += 1; // consume *
                self.expect_token(Token::As)?;
                let alias = self.expect_id()?;
                self.expect_token(Token::From)?;
                let source = self.parse_import_source()?;
                ImportStmt::Namespace { alias, source }
            }
            // use module; or use module as alias;
            Token::Id(_) => {
                let module = self.parse_module_path()?;
                if !self.eof() && self.tokens[self.pos] == Token::As {
                    self.pos += 1; // consume 'as'
                    let alias = self.expect_id()?;
                    ImportStmt::ModuleAlias { module, alias }
                } else {
                    ImportStmt::Module { module }
                }
            }
            _ => {
                return Err(anyhow!(self.err("Expected use specifier")));
            }
        };

        self.expect_token(Token::Semicolon)?;
        Ok(Stmt::Import(import_stmt))
    }

    fn parse_import_items(&mut self) -> Result<Vec<ImportItem>> {
        let mut items = Vec::new();

        loop {
            let name = self.expect_id()?;
            let alias = if !self.eof() && self.tokens[self.pos] == Token::As {
                self.pos += 1; // consume 'as'
                Some(self.expect_id()?)
            } else {
                None
            };

            items.push(ImportItem { name, alias });

            // Check for more items
            if !self.eof() && self.tokens[self.pos] == Token::Comma {
                self.pos += 1; // consume comma
            } else {
                break;
            }
        }

        Ok(items)
    }

    fn parse_import_source(&mut self) -> Result<ImportSource> {
        if self.eof() {
            return Err(anyhow!(self.err("Expected module name or file path after 'from'")));
        }

        match &self.tokens[self.pos] {
            Token::Str(path) => {
                let path = path.clone();
                self.pos += 1;
                Ok(ImportSource::File(path))
            }
            Token::Id(name) => {
                let name = self.parse_module_path_from_first(name.clone())?;
                Ok(ImportSource::Module(name))
            }
            _ => Err(anyhow!(self.err("Expected module name or file path"))),
        }
    }

    fn parse_module_path(&mut self) -> Result<String> {
        let Token::Id(first) = &self.tokens[self.pos] else {
            return Err(anyhow!(self.err("Expected module name")));
        };
        self.parse_module_path_from_first(first.clone())
    }

    fn parse_module_path_from_first(&mut self, first: String) -> Result<String> {
        self.pos += 1;
        Ok(first)
    }
}
