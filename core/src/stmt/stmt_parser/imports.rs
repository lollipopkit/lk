use super::StmtParser;
use crate::{
    stmt::{ImportItem, ImportSource, ImportStmt, Stmt},
    token::Token,
};
use anyhow::{Result, anyhow};

impl<'a> StmtParser<'a> {
    pub fn parse_import_stmt(&mut self) -> Result<Stmt> {
        self.expect_token(Token::Import)?;

        // After 'import', ensure there is a specifier
        if self.eof() {
            return Err(anyhow!(self.err("Expected import specifier after 'import'")));
        }

        // Check for different import patterns
        let import_stmt = match &self.tokens[self.pos] {
            // import "path";
            Token::Str(path) => {
                let path = path.clone();
                self.pos += 1;
                ImportStmt::File { path }
            }
            // import { ... } from source
            Token::LBrace => {
                self.pos += 1; // consume {
                let items = self.parse_import_items()?;
                self.expect_token(Token::RBrace)?;
                self.expect_token(Token::From)?;
                let source = self.parse_import_source()?;
                ImportStmt::Items { items, source }
            }
            // import * as alias from source
            Token::Mul => {
                self.pos += 1; // consume *
                self.expect_token(Token::As)?;
                let alias = self.expect_id()?;
                self.expect_token(Token::From)?;
                let source = self.parse_import_source()?;
                ImportStmt::Namespace { alias, source }
            }
            // import module; or import module as alias;
            Token::Id(module) => {
                let module = module.clone();
                self.pos += 1;

                if !self.eof() && self.tokens[self.pos] == Token::As {
                    self.pos += 1; // consume 'as'
                    let alias = self.expect_id()?;
                    ImportStmt::ModuleAlias { module, alias }
                } else {
                    ImportStmt::Module { module }
                }
            }
            _ => {
                return Err(anyhow!(self.err("Expected import specifier")));
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
                let name = name.clone();
                self.pos += 1;
                Ok(ImportSource::Module(name))
            }
            _ => Err(anyhow!(self.err("Expected module name or file path"))),
        }
    }
}
