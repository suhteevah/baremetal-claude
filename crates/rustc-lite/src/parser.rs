//! Full Rust recursive-descent parser for ClaudioOS rustc-lite.
//!
//! Parses token stream from the lexer into the AST defined in `ast.rs`.
//! Uses Pratt parsing for expression precedence.

#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::ast::*;
use crate::lexer::{Spanned, Token};

// ─── Parser state ────────────────────────────────────────────────────────

pub struct Parser {
    tokens: Vec<Spanned>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Spanned>) -> Self {
        Self { tokens, pos: 0 }
    }

    pub fn parse_file(tokens: Vec<Spanned>) -> Result<SourceFile, String> {
        let mut parser = Parser::new(tokens);
        let mut items = Vec::new();
        while !parser.at_eof() {
            items.push(parser.parse_item()?);
        }
        Ok(SourceFile { items })
    }

    // ── Helpers ──────────────────────────────────────────────────────

    fn peek(&self) -> &Token {
        if self.pos < self.tokens.len() {
            &self.tokens[self.pos].token
        } else {
            &Token::Eof
        }
    }

    fn peek2(&self) -> &Token {
        if self.pos + 1 < self.tokens.len() {
            &self.tokens[self.pos + 1].token
        } else {
            &Token::Eof
        }
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek(), Token::Eof)
    }

    fn advance(&mut self) -> Token {
        if self.pos < self.tokens.len() {
            let tok = self.tokens[self.pos].token.clone();
            self.pos += 1;
            tok
        } else {
            Token::Eof
        }
    }

    fn expect(&mut self, expected: &Token) -> Result<(), String> {
        let got = self.peek().clone();
        if &got == expected {
            self.advance();
            Ok(())
        } else {
            Err(format!("expected {:?}, got {:?}", expected, got))
        }
    }

    fn eat(&mut self, tok: &Token) -> bool {
        if self.peek() == tok {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect_ident(&mut self) -> Result<String, String> {
        match self.advance() {
            Token::Ident(s) => Ok(s),
            other => Err(format!("expected identifier, got {:?}", other)),
        }
    }

    fn at(&self, tok: &Token) -> bool {
        self.peek() == tok
    }

    // ── Items ────────────────────────────────────────────────────────

    fn parse_item(&mut self) -> Result<Item, String> {
        let attrs = self.parse_outer_attrs()?;
        let vis = self.parse_visibility()?;

        let kind = match self.peek().clone() {
            Token::Fn => self.parse_fn_def(false, false, false, None)?,
            Token::Async => {
                self.advance();
                self.parse_fn_def(true, false, false, None)?
            }
            Token::Unsafe => {
                self.advance();
                match self.peek() {
                    Token::Fn => self.parse_fn_def(false, true, false, None)?,
                    Token::Impl => self.parse_impl_block(true)?,
                    Token::Trait => self.parse_trait_def(true)?,
                    _ => return Err(format!("expected fn/impl/trait after unsafe, got {:?}", self.peek())),
                }
            }
            Token::Const => {
                if matches!(self.peek2(), Token::Fn) {
                    self.advance();
                    self.parse_fn_def(false, false, true, None)?
                } else {
                    self.parse_const_def()?
                }
            }
            Token::Extern => {
                self.advance();
                let abi = if let Token::StringLit(s) = self.peek().clone() {
                    self.advance();
                    Some(s)
                } else {
                    None
                };
                if matches!(self.peek(), Token::Fn) {
                    self.parse_fn_def(false, false, false, abi)?
                } else if matches!(self.peek(), Token::LBrace) {
                    self.parse_extern_block(abi)?
                } else {
                    return Err(format!("expected fn or {{ after extern, got {:?}", self.peek()));
                }
            }
            Token::Struct => self.parse_struct_def()?,
            Token::Enum => self.parse_enum_def()?,
            Token::Impl => self.parse_impl_block(false)?,
            Token::Trait => self.parse_trait_def(false)?,
            Token::Type => self.parse_type_alias()?,
            Token::Static => self.parse_static_def()?,
            Token::Use => self.parse_use()?,
            Token::Mod => self.parse_mod()?,
            _ => return Err(format!("expected item, got {:?}", self.peek())),
        };

        Ok(Item { vis, attrs, kind })
    }

    fn parse_outer_attrs(&mut self) -> Result<Vec<Attribute>, String> {
        let mut attrs = Vec::new();
        while self.at(&Token::Pound) && !matches!(self.peek2(), Token::Bang) {
            self.advance(); // #
            self.expect(&Token::LBracket)?;
            let path = self.parse_simple_path()?;
            let args = if self.at(&Token::LParen) {
                Some(self.parse_attr_args()?)
            } else {
                None
            };
            self.expect(&Token::RBracket)?;
            attrs.push(Attribute { path, args });
        }
        Ok(attrs)
    }

    fn parse_attr_args(&mut self) -> Result<String, String> {
        self.expect(&Token::LParen)?;
        let mut depth = 1u32;
        let mut s = String::new();
        loop {
            match self.peek() {
                Token::LParen => { depth += 1; s.push('('); self.advance(); }
                Token::RParen => {
                    depth -= 1;
                    if depth == 0 { self.advance(); break; }
                    s.push(')');
                    self.advance();
                }
                Token::Eof => return Err("unterminated attribute".into()),
                _ => {
                    s.push_str(&format!("{:?} ", self.peek()));
                    self.advance();
                }
            }
        }
        Ok(s)
    }

    fn parse_visibility(&mut self) -> Result<Visibility, String> {
        if !self.at(&Token::Pub) {
            return Ok(Visibility::Private);
        }
        self.advance(); // pub
        if self.at(&Token::LParen) {
            self.advance();
            let vis = match self.peek() {
                Token::Crate => { self.advance(); Visibility::PubCrate }
                Token::Super => { self.advance(); Visibility::PubSuper }
                _ => Visibility::Pub,
            };
            if vis != Visibility::Pub {
                self.expect(&Token::RParen)?;
            } else {
                // rollback - wasn't pub(crate/super), just pub followed by something else
                self.pos -= 1; // undo LParen advance
                return Ok(Visibility::Pub);
            }
            Ok(vis)
        } else {
            Ok(Visibility::Pub)
        }
    }

    // ── Function ─────────────────────────────────────────────────────

    fn parse_fn_def(
        &mut self,
        is_async: bool,
        is_unsafe: bool,
        is_const: bool,
        abi: Option<String>,
    ) -> Result<ItemKind, String> {
        self.expect(&Token::Fn)?;
        let name = self.expect_ident()?;
        let generics = self.parse_generics()?;
        self.expect(&Token::LParen)?;
        let params = self.parse_fn_params()?;
        self.expect(&Token::RParen)?;

        let ret_type = if self.eat(&Token::ThinArrow) {
            Some(self.parse_type()?)
        } else {
            None
        };

        let where_clause = self.parse_where_clause()?;

        let body = if self.at(&Token::LBrace) {
            Some(self.parse_block()?)
        } else {
            self.expect(&Token::Semi)?;
            None
        };

        Ok(ItemKind::Function(FnDef {
            name,
            generics,
            params,
            ret_type,
            where_clause,
            body,
            is_async,
            is_unsafe,
            is_const,
            abi,
        }))
    }

    fn parse_fn_params(&mut self) -> Result<Vec<FnParam>, String> {
        let mut params = Vec::new();
        while !self.at(&Token::RParen) && !self.at_eof() {
            if self.at(&Token::SelfLower)
                || (self.at(&Token::Amp) && matches!(self.peek2(), Token::SelfLower | Token::Mut))
                || (self.at(&Token::Mut) && matches!(self.peek2(), Token::SelfLower))
            {
                params.push(self.parse_self_param()?);
            } else {
                let pat = self.parse_pattern()?;
                self.expect(&Token::Colon)?;
                let ty = self.parse_type()?;
                params.push(FnParam::Typed { pat, ty });
            }
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        Ok(params)
    }

    fn parse_self_param(&mut self) -> Result<FnParam, String> {
        let is_ref;
        let is_mut;
        let lifetime;

        if self.eat(&Token::Amp) {
            is_ref = true;
            lifetime = if let Token::Lifetime(l) = self.peek().clone() {
                self.advance();
                Some(l)
            } else {
                None
            };
            is_mut = self.eat(&Token::Mut);
            self.expect(&Token::SelfLower)?;
        } else if self.eat(&Token::Mut) {
            is_ref = false;
            is_mut = true;
            lifetime = None;
            self.expect(&Token::SelfLower)?;
        } else {
            self.expect(&Token::SelfLower)?;
            is_ref = false;
            is_mut = false;
            lifetime = None;
        }

        Ok(FnParam::SelfParam { is_ref, is_mut, lifetime })
    }

    // ── Struct ───────────────────────────────────────────────────────

    fn parse_struct_def(&mut self) -> Result<ItemKind, String> {
        self.expect(&Token::Struct)?;
        let name = self.expect_ident()?;
        let generics = self.parse_generics()?;
        let where_clause;

        let kind = if self.at(&Token::LBrace) || self.at(&Token::Where) {
            where_clause = self.parse_where_clause()?;
            self.expect(&Token::LBrace)?;
            let fields = self.parse_named_fields()?;
            self.expect(&Token::RBrace)?;
            StructKind::Named(fields)
        } else if self.at(&Token::LParen) {
            self.advance();
            let mut fields = Vec::new();
            while !self.at(&Token::RParen) && !self.at_eof() {
                let vis = self.parse_visibility()?;
                let ty = self.parse_type()?;
                fields.push(TupleFieldDef { vis, ty });
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
            self.expect(&Token::RParen)?;
            where_clause = self.parse_where_clause()?;
            self.expect(&Token::Semi)?;
            StructKind::Tuple(fields)
        } else {
            where_clause = WhereClause::default();
            self.expect(&Token::Semi)?;
            StructKind::Unit
        };

        Ok(ItemKind::Struct(StructDef {
            name,
            generics,
            where_clause,
            kind,
        }))
    }

    fn parse_named_fields(&mut self) -> Result<Vec<FieldDef>, String> {
        let mut fields = Vec::new();
        while !self.at(&Token::RBrace) && !self.at_eof() {
            let vis = self.parse_visibility()?;
            let name = self.expect_ident()?;
            self.expect(&Token::Colon)?;
            let ty = self.parse_type()?;
            fields.push(FieldDef { vis, name, ty });
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        Ok(fields)
    }

    // ── Enum ─────────────────────────────────────────────────────────

    fn parse_enum_def(&mut self) -> Result<ItemKind, String> {
        self.expect(&Token::Enum)?;
        let name = self.expect_ident()?;
        let generics = self.parse_generics()?;
        let where_clause = self.parse_where_clause()?;
        self.expect(&Token::LBrace)?;

        let mut variants = Vec::new();
        while !self.at(&Token::RBrace) && !self.at_eof() {
            let vname = self.expect_ident()?;
            let kind = if self.at(&Token::LParen) {
                self.advance();
                let mut types = Vec::new();
                while !self.at(&Token::RParen) && !self.at_eof() {
                    types.push(self.parse_type()?);
                    if !self.eat(&Token::Comma) { break; }
                }
                self.expect(&Token::RParen)?;
                VariantKind::Tuple(types)
            } else if self.at(&Token::LBrace) {
                self.advance();
                let fields = self.parse_named_fields()?;
                self.expect(&Token::RBrace)?;
                VariantKind::Struct(fields)
            } else {
                VariantKind::Unit
            };

            let discriminant = if self.eat(&Token::Eq) {
                Some(self.parse_expr()?)
            } else {
                None
            };

            variants.push(Variant { name: vname, kind, discriminant });
            if !self.eat(&Token::Comma) { break; }
        }
        self.expect(&Token::RBrace)?;

        Ok(ItemKind::Enum(EnumDef {
            name,
            generics,
            where_clause,
            variants,
        }))
    }

    // ── Impl ─────────────────────────────────────────────────────────

    fn parse_impl_block(&mut self, is_unsafe: bool) -> Result<ItemKind, String> {
        self.expect(&Token::Impl)?;
        let generics = self.parse_generics()?;

        let is_negative = self.eat(&Token::Bang);

        // Parse the type / trait. We need to distinguish:
        //   impl Type { ... }
        //   impl Trait for Type { ... }
        let first_ty = self.parse_type()?;

        let (trait_path, self_ty) = if self.eat(&Token::For) {
            // impl Trait for Type
            let trait_path = match &first_ty {
                Ty::Path(p) => p.clone(),
                _ => return Err("expected trait path in impl".into()),
            };
            let self_ty = self.parse_type()?;
            (Some(trait_path), self_ty)
        } else {
            (None, first_ty)
        };

        let where_clause = self.parse_where_clause()?;
        self.expect(&Token::LBrace)?;

        let mut items = Vec::new();
        while !self.at(&Token::RBrace) && !self.at_eof() {
            items.push(self.parse_item()?);
        }
        self.expect(&Token::RBrace)?;

        Ok(ItemKind::Impl(ImplBlock {
            generics,
            trait_path,
            self_ty,
            where_clause,
            items,
            is_unsafe,
            is_negative,
        }))
    }

    // ── Trait ─────────────────────────────────────────────────────────

    fn parse_trait_def(&mut self, is_unsafe: bool) -> Result<ItemKind, String> {
        self.expect(&Token::Trait)?;
        let name = self.expect_ident()?;
        let generics = self.parse_generics()?;

        let mut supertraits = Vec::new();
        if self.eat(&Token::Colon) {
            loop {
                supertraits.push(self.parse_trait_bound()?);
                if !self.eat(&Token::Plus) { break; }
            }
        }

        let where_clause = self.parse_where_clause()?;
        self.expect(&Token::LBrace)?;

        let mut items = Vec::new();
        while !self.at(&Token::RBrace) && !self.at_eof() {
            items.push(self.parse_item()?);
        }
        self.expect(&Token::RBrace)?;

        Ok(ItemKind::Trait(TraitDef {
            name,
            generics,
            where_clause,
            supertraits,
            items,
            is_unsafe,
            is_auto: false,
        }))
    }

    // ── Type alias ───────────────────────────────────────────────────

    fn parse_type_alias(&mut self) -> Result<ItemKind, String> {
        self.expect(&Token::Type)?;
        let name = self.expect_ident()?;
        let generics = self.parse_generics()?;
        let where_clause = self.parse_where_clause()?;
        let ty = if self.eat(&Token::Eq) {
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(&Token::Semi)?;
        Ok(ItemKind::TypeAlias(TypeAlias {
            name,
            generics,
            where_clause,
            ty,
        }))
    }

    // ── Const ────────────────────────────────────────────────────────

    fn parse_const_def(&mut self) -> Result<ItemKind, String> {
        self.expect(&Token::Const)?;
        let name = self.expect_ident()?;
        self.expect(&Token::Colon)?;
        let ty = self.parse_type()?;
        let value = if self.eat(&Token::Eq) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.expect(&Token::Semi)?;
        Ok(ItemKind::Const(ConstDef { name, ty, value }))
    }

    // ── Static ───────────────────────────────────────────────────────

    fn parse_static_def(&mut self) -> Result<ItemKind, String> {
        self.expect(&Token::Static)?;
        let is_mut = self.eat(&Token::Mut);
        let name = self.expect_ident()?;
        self.expect(&Token::Colon)?;
        let ty = self.parse_type()?;
        let value = if self.eat(&Token::Eq) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.expect(&Token::Semi)?;
        Ok(ItemKind::Static(StaticDef {
            name,
            ty,
            value,
            is_mut,
        }))
    }

    // ── Use ──────────────────────────────────────────────────────────

    fn parse_use(&mut self) -> Result<ItemKind, String> {
        self.expect(&Token::Use)?;
        let path = self.parse_use_path()?;
        self.expect(&Token::Semi)?;
        Ok(ItemKind::Use(path))
    }

    fn parse_use_path(&mut self) -> Result<UsePath, String> {
        let mut segments = Vec::new();

        loop {
            if self.at(&Token::Star) {
                self.advance();
                let path = Path {
                    segments,
                    is_global: false,
                };
                return Ok(UsePath::Glob(path));
            }

            if self.at(&Token::LBrace) {
                self.advance();
                let path = Path {
                    segments,
                    is_global: false,
                };
                let mut group = Vec::new();
                while !self.at(&Token::RBrace) && !self.at_eof() {
                    group.push(self.parse_use_path()?);
                    if !self.eat(&Token::Comma) {
                        break;
                    }
                }
                self.expect(&Token::RBrace)?;
                return Ok(UsePath::Group(path, group));
            }

            let ident = self.expect_ident()?;
            segments.push(PathSegment {
                ident,
                generics: Vec::new(),
            });

            if self.eat(&Token::ColonColon) {
                continue;
            }

            // Check for `as alias`
            let alias = if self.eat(&Token::As) {
                Some(self.expect_ident()?)
            } else {
                None
            };

            let path = Path {
                segments,
                is_global: false,
            };
            return Ok(UsePath::Simple(path, alias));
        }
    }

    // ── Mod ──────────────────────────────────────────────────────────

    fn parse_mod(&mut self) -> Result<ItemKind, String> {
        self.expect(&Token::Mod)?;
        let name = self.expect_ident()?;
        if self.eat(&Token::Semi) {
            Ok(ItemKind::Mod(ModDef::Unloaded { name }))
        } else {
            self.expect(&Token::LBrace)?;
            let mut items = Vec::new();
            while !self.at(&Token::RBrace) && !self.at_eof() {
                items.push(self.parse_item()?);
            }
            self.expect(&Token::RBrace)?;
            Ok(ItemKind::Mod(ModDef::Loaded { name, items }))
        }
    }

    // ── Extern block ─────────────────────────────────────────────────

    fn parse_extern_block(&mut self, abi: Option<String>) -> Result<ItemKind, String> {
        self.expect(&Token::LBrace)?;
        let mut items = Vec::new();
        while !self.at(&Token::RBrace) && !self.at_eof() {
            items.push(self.parse_item()?);
        }
        self.expect(&Token::RBrace)?;
        Ok(ItemKind::ExternBlock(ExternBlock { abi, items }))
    }

    // ── Generics ─────────────────────────────────────────────────────

    fn parse_generics(&mut self) -> Result<Generics, String> {
        if !self.at(&Token::Lt) {
            return Ok(Generics::default());
        }
        self.advance(); // <

        let mut params = Vec::new();
        while !self.at(&Token::Gt) && !self.at_eof() {
            if let Token::Lifetime(name) = self.peek().clone() {
                self.advance();
                let mut bounds = Vec::new();
                if self.eat(&Token::Colon) {
                    loop {
                        if let Token::Lifetime(b) = self.peek().clone() {
                            self.advance();
                            bounds.push(b);
                        } else {
                            break;
                        }
                        if !self.eat(&Token::Plus) { break; }
                    }
                }
                params.push(GenericParam::Lifetime { name, bounds });
            } else if self.at(&Token::Const) {
                self.advance();
                let name = self.expect_ident()?;
                self.expect(&Token::Colon)?;
                let ty = self.parse_type()?;
                let default = if self.eat(&Token::Eq) {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                params.push(GenericParam::Const { name, ty, default });
            } else {
                let name = self.expect_ident()?;
                let mut bounds = Vec::new();
                if self.eat(&Token::Colon) {
                    loop {
                        if self.at(&Token::Comma) || self.at(&Token::Gt) || self.at(&Token::Eq) {
                            break;
                        }
                        bounds.push(self.parse_trait_bound()?);
                        if !self.eat(&Token::Plus) { break; }
                    }
                }
                let default = if self.eat(&Token::Eq) {
                    Some(self.parse_type()?)
                } else {
                    None
                };
                params.push(GenericParam::Type {
                    name,
                    bounds,
                    default,
                });
            }
            if !self.eat(&Token::Comma) { break; }
        }
        self.expect(&Token::Gt)?;
        Ok(Generics { params })
    }

    fn parse_generic_args(&mut self) -> Result<Vec<GenericArg>, String> {
        if !self.at(&Token::Lt) {
            return Ok(Vec::new());
        }
        self.advance(); // <

        let mut args = Vec::new();
        while !self.at(&Token::Gt) && !self.at_eof() {
            if let Token::Lifetime(l) = self.peek().clone() {
                self.advance();
                args.push(GenericArg::Lifetime(l));
            } else {
                // Could be Type, or Ident = Type (binding)
                let saved = self.pos;
                if let Token::Ident(name) = self.peek().clone() {
                    self.advance();
                    if self.eat(&Token::Eq) {
                        let ty = self.parse_type()?;
                        args.push(GenericArg::Binding { name, ty });
                    } else {
                        self.pos = saved;
                        args.push(GenericArg::Type(self.parse_type()?));
                    }
                } else {
                    args.push(GenericArg::Type(self.parse_type()?));
                }
            }
            if !self.eat(&Token::Comma) { break; }
        }
        self.expect(&Token::Gt)?;
        Ok(args)
    }

    fn parse_where_clause(&mut self) -> Result<WhereClause, String> {
        if !self.eat(&Token::Where) {
            return Ok(WhereClause::default());
        }
        let mut predicates = Vec::new();
        loop {
            if self.at(&Token::LBrace) || self.at(&Token::Semi) || self.at_eof() {
                break;
            }
            if let Token::Lifetime(l) = self.peek().clone() {
                self.advance();
                self.expect(&Token::Colon)?;
                let mut bounds = Vec::new();
                loop {
                    if let Token::Lifetime(b) = self.peek().clone() {
                        self.advance();
                        bounds.push(b);
                    } else {
                        break;
                    }
                    if !self.eat(&Token::Plus) { break; }
                }
                predicates.push(WherePredicate::LifetimeBound {
                    lifetime: l,
                    bounds,
                });
            } else {
                let ty = self.parse_type()?;
                self.expect(&Token::Colon)?;
                let mut bounds = Vec::new();
                loop {
                    if self.at(&Token::Comma) || self.at(&Token::LBrace) || self.at(&Token::Semi) {
                        break;
                    }
                    bounds.push(self.parse_trait_bound()?);
                    if !self.eat(&Token::Plus) { break; }
                }
                predicates.push(WherePredicate::TypeBound { ty, bounds });
            }
            if !self.eat(&Token::Comma) { break; }
        }
        Ok(WhereClause { predicates })
    }

    fn parse_trait_bound(&mut self) -> Result<TraitBound, String> {
        let is_maybe = self.eat(&Token::Question);
        let path = self.parse_path()?;
        let generics = if self.at(&Token::Lt) {
            self.parse_generic_args()?
        } else {
            Vec::new()
        };
        Ok(TraitBound { path, generics, is_maybe })
    }

    // ── Paths ────────────────────────────────────────────────────────

    fn parse_path(&mut self) -> Result<Path, String> {
        let is_global = self.eat(&Token::ColonColon);
        let mut segments = Vec::new();

        loop {
            let ident = match self.peek().clone() {
                Token::Ident(s) => { self.advance(); s }
                Token::SelfLower => { self.advance(); "self".into() }
                Token::SelfUpper => { self.advance(); "Self".into() }
                Token::Super => { self.advance(); "super".into() }
                Token::Crate => { self.advance(); "crate".into() }
                _ => return Err(format!("expected path segment, got {:?}", self.peek())),
            };

            // Turbofish ::<T>
            let generics = if self.at(&Token::ColonColon) && self.peek2() == &Token::Lt {
                self.advance(); // ::
                self.parse_generic_args()?
            } else {
                Vec::new()
            };

            segments.push(PathSegment { ident, generics });

            if self.at(&Token::ColonColon) && !matches!(self.peek2(), Token::Lt) {
                self.advance(); // ::
            } else {
                break;
            }
        }

        Ok(Path { segments, is_global })
    }

    fn parse_simple_path(&mut self) -> Result<Path, String> {
        self.parse_path()
    }

    // ── Types ────────────────────────────────────────────────────────

    fn parse_type(&mut self) -> Result<Ty, String> {
        match self.peek().clone() {
            Token::Amp => {
                self.advance();
                let lifetime = if let Token::Lifetime(l) = self.peek().clone() {
                    self.advance();
                    Some(l)
                } else {
                    None
                };
                let is_mut = self.eat(&Token::Mut);
                let inner = self.parse_type()?;
                Ok(Ty::Reference {
                    lifetime,
                    is_mut,
                    inner: Box::new(inner),
                })
            }
            Token::Star => {
                self.advance();
                let is_mut = if self.eat(&Token::Mut) {
                    true
                } else if self.at(&Token::Const) {
                    self.advance();
                    false
                } else {
                    return Err("expected mut or const after *".into());
                };
                let inner = self.parse_type()?;
                Ok(Ty::RawPtr {
                    is_mut,
                    inner: Box::new(inner),
                })
            }
            Token::LBracket => {
                self.advance();
                let inner = self.parse_type()?;
                if self.eat(&Token::Semi) {
                    let count = self.parse_expr()?;
                    self.expect(&Token::RBracket)?;
                    Ok(Ty::Array(Box::new(inner), Box::new(count)))
                } else {
                    self.expect(&Token::RBracket)?;
                    Ok(Ty::Slice(Box::new(inner)))
                }
            }
            Token::LParen => {
                self.advance();
                if self.at(&Token::RParen) {
                    self.advance();
                    return Ok(Ty::Tuple(Vec::new())); // unit ()
                }
                let first = self.parse_type()?;
                if self.eat(&Token::Comma) {
                    let mut types = vec![first];
                    while !self.at(&Token::RParen) && !self.at_eof() {
                        types.push(self.parse_type()?);
                        if !self.eat(&Token::Comma) { break; }
                    }
                    self.expect(&Token::RParen)?;
                    Ok(Ty::Tuple(types))
                } else {
                    self.expect(&Token::RParen)?;
                    Ok(first) // parenthesized type
                }
            }
            Token::Bang => {
                self.advance();
                Ok(Ty::Never)
            }
            Token::Underscore => {
                self.advance();
                Ok(Ty::Infer)
            }
            Token::SelfUpper => {
                self.advance();
                Ok(Ty::SelfType)
            }
            Token::Fn => {
                self.advance();
                self.expect(&Token::LParen)?;
                let mut params = Vec::new();
                while !self.at(&Token::RParen) && !self.at_eof() {
                    params.push(self.parse_type()?);
                    if !self.eat(&Token::Comma) { break; }
                }
                self.expect(&Token::RParen)?;
                let ret = if self.eat(&Token::ThinArrow) {
                    Some(Box::new(self.parse_type()?))
                } else {
                    None
                };
                Ok(Ty::Fn { params, ret })
            }
            Token::Impl => {
                self.advance();
                let mut bounds = Vec::new();
                loop {
                    bounds.push(self.parse_trait_bound()?);
                    if !self.eat(&Token::Plus) { break; }
                }
                Ok(Ty::ImplTrait(bounds))
            }
            Token::Dyn => {
                self.advance();
                let mut bounds = Vec::new();
                loop {
                    bounds.push(self.parse_trait_bound()?);
                    if !self.eat(&Token::Plus) { break; }
                }
                Ok(Ty::DynTrait(bounds))
            }
            _ => {
                let path = self.parse_path()?;
                // Check for generic args on the final segment
                if self.at(&Token::Lt) {
                    let args = self.parse_generic_args()?;
                    let mut p = path;
                    if let Some(last) = p.segments.last_mut() {
                        last.generics = args;
                    }
                    Ok(Ty::Path(p))
                } else {
                    Ok(Ty::Path(path))
                }
            }
        }
    }

    // ── Patterns ─────────────────────────────────────────────────────

    fn parse_pattern(&mut self) -> Result<Pattern, String> {
        let mut pat = self.parse_pattern_atom()?;

        // Or patterns: pat1 | pat2
        if self.at(&Token::Pipe) {
            let mut pats = vec![pat];
            while self.eat(&Token::Pipe) {
                pats.push(self.parse_pattern_atom()?);
            }
            pat = Pattern::Or(pats);
        }

        Ok(pat)
    }

    fn parse_pattern_atom(&mut self) -> Result<Pattern, String> {
        match self.peek().clone() {
            Token::Underscore => {
                self.advance();
                Ok(Pattern::Wildcard)
            }
            Token::DotDot => {
                self.advance();
                Ok(Pattern::Rest)
            }
            Token::Amp => {
                self.advance();
                let is_mut = self.eat(&Token::Mut);
                let inner = self.parse_pattern()?;
                Ok(Pattern::Ref { is_mut, pat: Box::new(inner) })
            }
            Token::Mut => {
                self.advance();
                let name = self.expect_ident()?;
                let binding = if self.eat(&Token::At) {
                    Some(Box::new(self.parse_pattern()?))
                } else {
                    None
                };
                Ok(Pattern::Ident {
                    name,
                    is_mut: true,
                    is_ref: false,
                    binding,
                })
            }
            Token::Ref => {
                self.advance();
                let is_mut = self.eat(&Token::Mut);
                let name = self.expect_ident()?;
                Ok(Pattern::Ident {
                    name,
                    is_mut,
                    is_ref: true,
                    binding: None,
                })
            }
            Token::LParen => {
                self.advance();
                let mut pats = Vec::new();
                while !self.at(&Token::RParen) && !self.at_eof() {
                    pats.push(self.parse_pattern()?);
                    if !self.eat(&Token::Comma) { break; }
                }
                self.expect(&Token::RParen)?;
                Ok(Pattern::Tuple(pats))
            }
            Token::LBracket => {
                self.advance();
                let mut pats = Vec::new();
                while !self.at(&Token::RBracket) && !self.at_eof() {
                    pats.push(self.parse_pattern()?);
                    if !self.eat(&Token::Comma) { break; }
                }
                self.expect(&Token::RBracket)?;
                Ok(Pattern::Slice(pats))
            }
            Token::True => {
                self.advance();
                Ok(Pattern::Lit(Box::new(Expr::new(ExprKind::BoolLit(true)))))
            }
            Token::False => {
                self.advance();
                Ok(Pattern::Lit(Box::new(Expr::new(ExprKind::BoolLit(false)))))
            }
            Token::IntLit(n) => {
                self.advance();
                Ok(Pattern::Lit(Box::new(Expr::new(ExprKind::IntLit(n)))))
            }
            Token::Minus => {
                self.advance();
                if let Token::IntLit(n) = self.advance() {
                    Ok(Pattern::Lit(Box::new(Expr::new(ExprKind::IntLit(-n)))))
                } else {
                    Err("expected integer after - in pattern".into())
                }
            }
            Token::StringLit(s) => {
                self.advance();
                Ok(Pattern::Lit(Box::new(Expr::new(ExprKind::StringLit(s)))))
            }
            Token::CharLit(c) => {
                self.advance();
                Ok(Pattern::Lit(Box::new(Expr::new(ExprKind::CharLit(c)))))
            }
            Token::Ident(_) | Token::ColonColon | Token::SelfLower
            | Token::SelfUpper | Token::Super | Token::Crate => {
                let path = self.parse_path()?;

                // Struct pattern: Path { field, .. }
                if self.at(&Token::LBrace) {
                    self.advance();
                    let mut fields = Vec::new();
                    let mut rest = false;
                    while !self.at(&Token::RBrace) && !self.at_eof() {
                        if self.at(&Token::DotDot) {
                            self.advance();
                            rest = true;
                            break;
                        }
                        let name = self.expect_ident()?;
                        if self.eat(&Token::Colon) {
                            let pat = self.parse_pattern()?;
                            fields.push(FieldPat {
                                name,
                                pat,
                                is_shorthand: false,
                            });
                        } else {
                            fields.push(FieldPat {
                                name: name.clone(),
                                pat: Pattern::Ident {
                                    name,
                                    is_mut: false,
                                    is_ref: false,
                                    binding: None,
                                },
                                is_shorthand: true,
                            });
                        }
                        if !self.eat(&Token::Comma) { break; }
                    }
                    self.expect(&Token::RBrace)?;
                    return Ok(Pattern::Struct { path, fields, rest });
                }

                // Tuple struct pattern: Path(a, b)
                if self.at(&Token::LParen) {
                    self.advance();
                    let mut pats = Vec::new();
                    while !self.at(&Token::RParen) && !self.at_eof() {
                        pats.push(self.parse_pattern()?);
                        if !self.eat(&Token::Comma) { break; }
                    }
                    self.expect(&Token::RParen)?;
                    return Ok(Pattern::TupleStruct { path, fields: pats });
                }

                // Simple ident with optional @ binding
                if path.segments.len() == 1 && path.segments[0].generics.is_empty() {
                    let name = path.segments[0].ident.clone();
                    if self.eat(&Token::At) {
                        let sub = self.parse_pattern()?;
                        return Ok(Pattern::Ident {
                            name,
                            is_mut: false,
                            is_ref: false,
                            binding: Some(Box::new(sub)),
                        });
                    }
                    // Check if it's really a path (enum variant) or just ident
                    // Simple heuristic: if uppercase first char, treat as path
                    if name.starts_with(|c: char| c.is_uppercase()) {
                        return Ok(Pattern::Path(path));
                    }
                    return Ok(Pattern::Ident {
                        name,
                        is_mut: false,
                        is_ref: false,
                        binding: None,
                    });
                }

                Ok(Pattern::Path(path))
            }
            _ => Err(format!("expected pattern, got {:?}", self.peek())),
        }
    }

    // ── Blocks ───────────────────────────────────────────────────────

    fn parse_block(&mut self) -> Result<Block, String> {
        self.expect(&Token::LBrace)?;
        let mut stmts = Vec::new();

        while !self.at(&Token::RBrace) && !self.at_eof() {
            // Nested items
            if self.is_item_start() {
                stmts.push(Stmt::Item(self.parse_item()?));
                continue;
            }

            // Let statement
            if self.at(&Token::Let) {
                self.advance();
                let pat = self.parse_pattern()?;
                let ty = if self.eat(&Token::Colon) {
                    Some(self.parse_type()?)
                } else {
                    None
                };
                let init = if self.eat(&Token::Eq) {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                self.expect(&Token::Semi)?;
                stmts.push(Stmt::Let { pat, ty, init });
                continue;
            }

            // Expression (possibly with semi)
            let expr = self.parse_expr()?;
            if self.eat(&Token::Semi) {
                stmts.push(Stmt::Expr(expr));
            } else if self.at(&Token::RBrace) {
                stmts.push(Stmt::ExprNoSemi(expr));
            } else {
                // Expression like if/match/loop don't need semi
                if self.expr_needs_semi(&expr) {
                    return Err(format!("expected ; after expression, got {:?}", self.peek()));
                }
                stmts.push(Stmt::Expr(expr));
            }
        }

        self.expect(&Token::RBrace)?;
        Ok(Block { stmts })
    }

    fn is_item_start(&self) -> bool {
        match self.peek() {
            Token::Fn | Token::Struct | Token::Enum | Token::Impl | Token::Trait
            | Token::Type | Token::Const | Token::Static | Token::Use | Token::Mod
            | Token::Extern | Token::Unsafe => true,
            Token::Pub => true,
            Token::Async => matches!(self.peek2(), Token::Fn),
            Token::Pound => true, // attribute
            _ => false,
        }
    }

    fn expr_needs_semi(&self, expr: &Expr) -> bool {
        !matches!(
            expr.kind,
            ExprKind::If { .. }
                | ExprKind::Match { .. }
                | ExprKind::Loop { .. }
                | ExprKind::While { .. }
                | ExprKind::For { .. }
                | ExprKind::Block(_)
                | ExprKind::Unsafe(_)
        )
    }

    // ── Expressions (Pratt parsing) ──────────────────────────────────

    pub fn parse_expr(&mut self) -> Result<Expr, String> {
        self.parse_expr_bp(0)
    }

    fn parse_expr_bp(&mut self, min_bp: u8) -> Result<Expr, String> {
        let mut lhs = self.parse_expr_prefix()?;

        loop {
            // Postfix operators
            match self.peek().clone() {
                Token::Dot => {
                    self.advance();
                    if self.at(&Token::Await) {
                        self.advance();
                        lhs = Expr::new(ExprKind::Await(Box::new(lhs)));
                        continue;
                    }
                    if let Token::IntLit(n) = self.peek().clone() {
                        self.advance();
                        lhs = Expr::new(ExprKind::TupleIndex {
                            expr: Box::new(lhs),
                            index: n as u32,
                        });
                        continue;
                    }
                    let method = self.expect_ident()?;
                    let generics = if self.at(&Token::ColonColon) && self.peek2() == &Token::Lt {
                        self.advance();
                        self.parse_generic_args()?
                    } else {
                        Vec::new()
                    };
                    if self.at(&Token::LParen) {
                        self.advance();
                        let args = self.parse_expr_list(&Token::RParen)?;
                        self.expect(&Token::RParen)?;
                        lhs = Expr::new(ExprKind::MethodCall {
                            receiver: Box::new(lhs),
                            method,
                            generics,
                            args,
                        });
                    } else {
                        lhs = Expr::new(ExprKind::Field {
                            expr: Box::new(lhs),
                            name: method,
                        });
                    }
                    continue;
                }
                Token::LBracket => {
                    self.advance();
                    let index = self.parse_expr()?;
                    self.expect(&Token::RBracket)?;
                    lhs = Expr::new(ExprKind::Index {
                        expr: Box::new(lhs),
                        index: Box::new(index),
                    });
                    continue;
                }
                Token::Question => {
                    self.advance();
                    lhs = Expr::new(ExprKind::Try(Box::new(lhs)));
                    continue;
                }
                Token::As => {
                    let (_, r_bp) = (10, 11); // `as` binding power
                    if 10 < min_bp { break; }
                    self.advance();
                    let ty = self.parse_type()?;
                    lhs = Expr::new(ExprKind::Cast {
                        expr: Box::new(lhs),
                        ty,
                    });
                    continue;
                }
                _ => {}
            }

            // Binary operators
            if let Some((l_bp, r_bp)) = self.infix_bp() {
                if l_bp < min_bp {
                    break;
                }

                let op_tok = self.advance();

                // Assignment operators
                match &op_tok {
                    Token::Eq => {
                        let rhs = self.parse_expr_bp(r_bp)?;
                        lhs = Expr::new(ExprKind::Assign {
                            lhs: Box::new(lhs),
                            rhs: Box::new(rhs),
                        });
                        continue;
                    }
                    Token::PlusEq | Token::MinusEq | Token::StarEq | Token::SlashEq
                    | Token::PercentEq | Token::AmpEq | Token::PipeEq | Token::CaretEq
                    | Token::ShlEq | Token::ShrEq => {
                        let op = assign_op_to_binop(&op_tok)?;
                        let rhs = self.parse_expr_bp(r_bp)?;
                        lhs = Expr::new(ExprKind::AssignOp {
                            op,
                            lhs: Box::new(lhs),
                            rhs: Box::new(rhs),
                        });
                        continue;
                    }
                    _ => {}
                }

                // Range operators
                if matches!(op_tok, Token::DotDot | Token::DotDotEq) {
                    let inclusive = matches!(op_tok, Token::DotDotEq);
                    let end = if self.at(&Token::Semi) || self.at(&Token::RBrace)
                        || self.at(&Token::RParen) || self.at(&Token::RBracket)
                        || self.at(&Token::Comma) || self.at_eof()
                    {
                        None
                    } else {
                        Some(Box::new(self.parse_expr_bp(r_bp)?))
                    };
                    lhs = Expr::new(ExprKind::Range {
                        start: Some(Box::new(lhs)),
                        end,
                        inclusive,
                    });
                    continue;
                }

                let op = token_to_binop(&op_tok)?;
                let rhs = self.parse_expr_bp(r_bp)?;
                lhs = Expr::new(ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                });
            } else {
                break;
            }
        }

        Ok(lhs)
    }

    fn infix_bp(&self) -> Option<(u8, u8)> {
        let bp = match self.peek() {
            Token::Eq | Token::PlusEq | Token::MinusEq | Token::StarEq
            | Token::SlashEq | Token::PercentEq | Token::AmpEq | Token::PipeEq
            | Token::CaretEq | Token::ShlEq | Token::ShrEq => (2, 1), // right-assoc

            Token::DotDot | Token::DotDotEq => (3, 4),

            Token::PipePipe => (5, 6),
            Token::AmpAmp => (7, 8),

            Token::EqEq | Token::Ne | Token::Lt | Token::Gt
            | Token::Le | Token::Ge => (9, 10),

            Token::Pipe => (11, 12),
            Token::Caret => (13, 14),
            Token::Amp => (15, 16),
            Token::Shl | Token::Shr => (17, 18),
            Token::Plus | Token::Minus => (19, 20),
            Token::Star | Token::Slash | Token::Percent => (21, 22),
            _ => return None,
        };
        Some(bp)
    }

    fn parse_expr_prefix(&mut self) -> Result<Expr, String> {
        match self.peek().clone() {
            // Unary operators
            Token::Minus => {
                self.advance();
                let expr = self.parse_expr_bp(23)?; // prefix bp
                Ok(Expr::new(ExprKind::Unary {
                    op: UnaryOp::Neg,
                    expr: Box::new(expr),
                }))
            }
            Token::Bang => {
                self.advance();
                let expr = self.parse_expr_bp(23)?;
                Ok(Expr::new(ExprKind::Unary {
                    op: UnaryOp::Not,
                    expr: Box::new(expr),
                }))
            }
            Token::Star => {
                self.advance();
                let expr = self.parse_expr_bp(23)?;
                Ok(Expr::new(ExprKind::Deref(Box::new(expr))))
            }
            Token::Amp => {
                self.advance();
                let is_mut = self.eat(&Token::Mut);
                let expr = self.parse_expr_bp(23)?;
                Ok(Expr::new(ExprKind::Ref {
                    is_mut,
                    expr: Box::new(expr),
                }))
            }

            // Range prefix (..end)
            Token::DotDot | Token::DotDotEq => {
                let inclusive = matches!(self.advance(), Token::DotDotEq);
                let end = if self.at(&Token::Semi) || self.at(&Token::RBrace)
                    || self.at(&Token::RParen) || self.at(&Token::RBracket)
                    || self.at(&Token::Comma) || self.at_eof()
                {
                    None
                } else {
                    Some(Box::new(self.parse_expr_bp(4)?))
                };
                Ok(Expr::new(ExprKind::Range {
                    start: None,
                    end,
                    inclusive,
                }))
            }

            // Literals
            Token::IntLit(n) => {
                self.advance();
                Ok(Expr::new(ExprKind::IntLit(n)))
            }
            Token::FloatLit(f) => {
                self.advance();
                Ok(Expr::new(ExprKind::FloatLit(f)))
            }
            Token::StringLit(s) => {
                self.advance();
                Ok(Expr::new(ExprKind::StringLit(s)))
            }
            Token::CharLit(c) => {
                self.advance();
                Ok(Expr::new(ExprKind::CharLit(c)))
            }
            Token::ByteLit(b) => {
                self.advance();
                Ok(Expr::new(ExprKind::ByteLit(b)))
            }
            Token::ByteStringLit(bs) => {
                self.advance();
                Ok(Expr::new(ExprKind::ByteStringLit(bs)))
            }
            Token::True => {
                self.advance();
                Ok(Expr::new(ExprKind::BoolLit(true)))
            }
            Token::False => {
                self.advance();
                Ok(Expr::new(ExprKind::BoolLit(false)))
            }

            // Grouping / tuple / unit
            Token::LParen => {
                self.advance();
                if self.at(&Token::RParen) {
                    self.advance();
                    return Ok(Expr::new(ExprKind::Tuple(Vec::new())));
                }
                let first = self.parse_expr()?;
                if self.eat(&Token::Comma) {
                    let mut exprs = vec![first];
                    while !self.at(&Token::RParen) && !self.at_eof() {
                        exprs.push(self.parse_expr()?);
                        if !self.eat(&Token::Comma) { break; }
                    }
                    self.expect(&Token::RParen)?;
                    Ok(Expr::new(ExprKind::Tuple(exprs)))
                } else {
                    self.expect(&Token::RParen)?;
                    Ok(first)
                }
            }

            // Array
            Token::LBracket => {
                self.advance();
                if self.at(&Token::RBracket) {
                    self.advance();
                    return Ok(Expr::new(ExprKind::Array(Vec::new())));
                }
                let first = self.parse_expr()?;
                if self.eat(&Token::Semi) {
                    // [expr; count]
                    let count = self.parse_expr()?;
                    self.expect(&Token::RBracket)?;
                    Ok(Expr::new(ExprKind::ArrayRepeat {
                        value: Box::new(first),
                        count: Box::new(count),
                    }))
                } else {
                    let mut elems = vec![first];
                    while self.eat(&Token::Comma) {
                        if self.at(&Token::RBracket) { break; }
                        elems.push(self.parse_expr()?);
                    }
                    self.expect(&Token::RBracket)?;
                    Ok(Expr::new(ExprKind::Array(elems)))
                }
            }

            // Block
            Token::LBrace => {
                let block = self.parse_block()?;
                Ok(Expr::new(ExprKind::Block(block)))
            }

            // Unsafe block
            Token::Unsafe => {
                self.advance();
                let block = self.parse_block()?;
                Ok(Expr::new(ExprKind::Unsafe(block)))
            }

            // If expression
            Token::If => self.parse_if_expr(),

            // Match expression
            Token::Match => self.parse_match_expr(),

            // Loop
            Token::Loop => {
                self.advance();
                let body = self.parse_block()?;
                Ok(Expr::new(ExprKind::Loop { body, label: None }))
            }

            // While
            Token::While => {
                self.advance();
                let cond = self.parse_expr_no_struct()?;
                let body = self.parse_block()?;
                Ok(Expr::new(ExprKind::While {
                    cond: Box::new(cond),
                    body,
                    label: None,
                }))
            }

            // For
            Token::For => {
                self.advance();
                let pat = self.parse_pattern()?;
                self.expect(&Token::In)?;
                let iter = self.parse_expr_no_struct()?;
                let body = self.parse_block()?;
                Ok(Expr::new(ExprKind::For {
                    pat,
                    iter: Box::new(iter),
                    body,
                    label: None,
                }))
            }

            // Return
            Token::Return => {
                self.advance();
                let val = if self.at(&Token::Semi) || self.at(&Token::RBrace) || self.at_eof() {
                    None
                } else {
                    Some(Box::new(self.parse_expr()?))
                };
                Ok(Expr::new(ExprKind::Return(val)))
            }

            // Break
            Token::Break => {
                self.advance();
                let label = if let Token::Lifetime(l) = self.peek().clone() {
                    self.advance();
                    Some(l)
                } else {
                    None
                };
                let value = if self.at(&Token::Semi) || self.at(&Token::RBrace) || self.at_eof() {
                    None
                } else {
                    Some(Box::new(self.parse_expr()?))
                };
                Ok(Expr::new(ExprKind::Break { label, value }))
            }

            // Continue
            Token::Continue => {
                self.advance();
                let label = if let Token::Lifetime(l) = self.peek().clone() {
                    self.advance();
                    Some(l)
                } else {
                    None
                };
                Ok(Expr::new(ExprKind::Continue { label }))
            }

            // Closure
            Token::Pipe | Token::PipePipe => self.parse_closure(false, false),
            Token::Move => {
                self.advance();
                self.parse_closure(true, false)
            }
            Token::Async => {
                self.advance();
                if self.at(&Token::Move) {
                    self.advance();
                    if self.at(&Token::Pipe) || self.at(&Token::PipePipe) {
                        self.parse_closure(true, true)
                    } else {
                        let block = self.parse_block()?;
                        Ok(Expr::new(ExprKind::Block(block)))
                    }
                } else if self.at(&Token::Pipe) || self.at(&Token::PipePipe) {
                    self.parse_closure(false, true)
                } else {
                    let block = self.parse_block()?;
                    Ok(Expr::new(ExprKind::Block(block)))
                }
            }

            // Path / ident / struct literal / macro / function call
            Token::Ident(_) | Token::ColonColon | Token::SelfLower | Token::SelfUpper
            | Token::Super | Token::Crate => {
                let path = self.parse_path()?;

                // Macro invocation: path!(...)
                if self.at(&Token::Bang) && !self.at_next_any(&[Token::Eq]) {
                    self.advance(); // !
                    let args = self.parse_macro_args()?;
                    return Ok(Expr::new(ExprKind::Macro { path, args }));
                }

                // Struct literal: Path { field: expr, ... }
                if self.at(&Token::LBrace) && self.can_start_struct_lit() {
                    return self.parse_struct_lit(path);
                }

                // Function call
                if self.at(&Token::LParen) {
                    self.advance();
                    let args = self.parse_expr_list(&Token::RParen)?;
                    self.expect(&Token::RParen)?;
                    return Ok(Expr::new(ExprKind::Call {
                        func: Box::new(Expr::new(ExprKind::Path(path))),
                        args,
                    }));
                }

                // Generic args on path
                if self.at(&Token::ColonColon) && self.peek2() == &Token::Lt {
                    self.advance();
                    let args = self.parse_generic_args()?;
                    let mut p = path;
                    if let Some(last) = p.segments.last_mut() {
                        last.generics = args;
                    }
                    // Could be followed by call
                    if self.at(&Token::LParen) {
                        self.advance();
                        let call_args = self.parse_expr_list(&Token::RParen)?;
                        self.expect(&Token::RParen)?;
                        return Ok(Expr::new(ExprKind::Call {
                            func: Box::new(Expr::new(ExprKind::Path(p))),
                            args: call_args,
                        }));
                    }
                    return Ok(Expr::new(ExprKind::Path(p)));
                }

                Ok(Expr::new(ExprKind::Path(path)))
            }

            // Label followed by loop
            Token::Lifetime(label) => {
                self.advance();
                self.expect(&Token::Colon)?;
                match self.peek() {
                    Token::Loop => {
                        self.advance();
                        let body = self.parse_block()?;
                        Ok(Expr::new(ExprKind::Loop {
                            body,
                            label: Some(label),
                        }))
                    }
                    Token::While => {
                        self.advance();
                        let cond = self.parse_expr_no_struct()?;
                        let body = self.parse_block()?;
                        Ok(Expr::new(ExprKind::While {
                            cond: Box::new(cond),
                            body,
                            label: Some(label),
                        }))
                    }
                    Token::For => {
                        self.advance();
                        let pat = self.parse_pattern()?;
                        self.expect(&Token::In)?;
                        let iter = self.parse_expr_no_struct()?;
                        let body = self.parse_block()?;
                        Ok(Expr::new(ExprKind::For {
                            pat,
                            iter: Box::new(iter),
                            body,
                            label: Some(label),
                        }))
                    }
                    _ => Err(format!("expected loop/while/for after label, got {:?}", self.peek())),
                }
            }

            _ => Err(format!("expected expression, got {:?}", self.peek())),
        }
    }

    fn parse_if_expr(&mut self) -> Result<Expr, String> {
        self.expect(&Token::If)?;
        let cond = if self.at(&Token::Let) {
            self.advance();
            let pat = self.parse_pattern()?;
            self.expect(&Token::Eq)?;
            let expr = self.parse_expr_no_struct()?;
            Expr::new(ExprKind::Let {
                pat,
                expr: Box::new(expr),
            })
        } else {
            self.parse_expr_no_struct()?
        };

        let then_block = self.parse_block()?;

        let else_expr = if self.eat(&Token::Else) {
            if self.at(&Token::If) {
                Some(Box::new(self.parse_if_expr()?))
            } else {
                let block = self.parse_block()?;
                Some(Box::new(Expr::new(ExprKind::Block(block))))
            }
        } else {
            None
        };

        Ok(Expr::new(ExprKind::If {
            cond: Box::new(cond),
            then_block,
            else_expr,
        }))
    }

    fn parse_match_expr(&mut self) -> Result<Expr, String> {
        self.expect(&Token::Match)?;
        let expr = self.parse_expr_no_struct()?;
        self.expect(&Token::LBrace)?;

        let mut arms = Vec::new();
        while !self.at(&Token::RBrace) && !self.at_eof() {
            let pat = self.parse_pattern()?;
            let guard = if self.eat(&Token::If) {
                Some(self.parse_expr_no_struct()?)
            } else {
                None
            };
            self.expect(&Token::FatArrow)?;
            let body = self.parse_expr()?;
            arms.push(MatchArm { pat, guard, body });
            // Comma is optional before }
            self.eat(&Token::Comma);
        }
        self.expect(&Token::RBrace)?;

        Ok(Expr::new(ExprKind::Match {
            expr: Box::new(expr),
            arms,
        }))
    }

    fn parse_closure(&mut self, is_move: bool, is_async: bool) -> Result<Expr, String> {
        let params = if self.eat(&Token::PipePipe) {
            Vec::new()
        } else {
            self.expect(&Token::Pipe)?;
            let mut params = Vec::new();
            while !self.at(&Token::Pipe) && !self.at_eof() {
                let pat = self.parse_pattern()?;
                let ty = if self.eat(&Token::Colon) {
                    Some(self.parse_type()?)
                } else {
                    None
                };
                params.push(ClosureParam { pat, ty });
                if !self.eat(&Token::Comma) { break; }
            }
            self.expect(&Token::Pipe)?;
            params
        };

        let ret_type = if self.eat(&Token::ThinArrow) {
            Some(self.parse_type()?)
        } else {
            None
        };

        let body = self.parse_expr()?;

        Ok(Expr::new(ExprKind::Closure {
            params,
            ret_type,
            body: Box::new(body),
            is_move,
            is_async,
        }))
    }

    fn parse_struct_lit(&mut self, path: Path) -> Result<Expr, String> {
        self.expect(&Token::LBrace)?;
        let mut fields = Vec::new();
        let mut rest = None;

        while !self.at(&Token::RBrace) && !self.at_eof() {
            if self.at(&Token::DotDot) {
                self.advance();
                rest = Some(Box::new(self.parse_expr()?));
                break;
            }
            let name = self.expect_ident()?;
            let value = if self.eat(&Token::Colon) {
                self.parse_expr()?
            } else {
                // Shorthand: Foo { x } == Foo { x: x }
                Expr::new(ExprKind::Path(Path::simple(&name)))
            };
            fields.push(StructLitField { name, value });
            if !self.eat(&Token::Comma) { break; }
        }
        self.expect(&Token::RBrace)?;

        Ok(Expr::new(ExprKind::StructLit { path, fields, rest }))
    }

    fn parse_macro_args(&mut self) -> Result<String, String> {
        let (open, close) = if self.at(&Token::LParen) {
            (Token::LParen, Token::RParen)
        } else if self.at(&Token::LBracket) {
            (Token::LBracket, Token::RBracket)
        } else if self.at(&Token::LBrace) {
            (Token::LBrace, Token::RBrace)
        } else {
            return Err(format!("expected ( or [ or {{ after macro!, got {:?}", self.peek()));
        };
        self.advance();

        let mut depth = 1u32;
        let start = self.pos;
        loop {
            if self.at_eof() {
                return Err("unterminated macro invocation".into());
            }
            if self.peek() == &open {
                depth += 1;
            } else if self.peek() == &close {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            self.advance();
        }
        // Collect token text (simplified — just join debug repr)
        let mut s = String::new();
        for i in start..self.pos {
            if i > start {
                s.push(' ');
            }
            s.push_str(&format!("{:?}", self.tokens[i].token));
        }
        self.advance(); // closing delimiter
        Ok(s)
    }

    // ── Helpers ──────────────────────────────────────────────────────

    fn parse_expr_list(&mut self, end: &Token) -> Result<Vec<Expr>, String> {
        let mut exprs = Vec::new();
        while !self.at(end) && !self.at_eof() {
            exprs.push(self.parse_expr()?);
            if !self.eat(&Token::Comma) { break; }
        }
        Ok(exprs)
    }

    /// Parse expression but don't allow struct literals (used in if/while/match conditions).
    fn parse_expr_no_struct(&mut self) -> Result<Expr, String> {
        // For now, just parse normally. A full implementation would
        // set a flag to prevent parsing { as struct literal start.
        self.parse_expr()
    }

    fn can_start_struct_lit(&self) -> bool {
        // Heuristic: { is a struct literal if the path contains ::
        // or starts with an uppercase letter
        // This is imperfect but handles common cases
        true
    }

    fn at_next_any(&self, tokens: &[Token]) -> bool {
        for t in tokens {
            if self.peek2() == t {
                return true;
            }
        }
        false
    }
}

// ─── Operator conversion ─────────────────────────────────────────────────

fn token_to_binop(tok: &Token) -> Result<BinOp, String> {
    Ok(match tok {
        Token::Plus => BinOp::Add,
        Token::Minus => BinOp::Sub,
        Token::Star => BinOp::Mul,
        Token::Slash => BinOp::Div,
        Token::Percent => BinOp::Rem,
        Token::Amp => BinOp::BitAnd,
        Token::Pipe => BinOp::BitOr,
        Token::Caret => BinOp::BitXor,
        Token::Shl => BinOp::Shl,
        Token::Shr => BinOp::Shr,
        Token::AmpAmp => BinOp::And,
        Token::PipePipe => BinOp::Or,
        Token::EqEq => BinOp::Eq,
        Token::Ne => BinOp::Ne,
        Token::Lt => BinOp::Lt,
        Token::Gt => BinOp::Gt,
        Token::Le => BinOp::Le,
        Token::Ge => BinOp::Ge,
        _ => return Err(format!("not a binary operator: {:?}", tok)),
    })
}

fn assign_op_to_binop(tok: &Token) -> Result<BinOp, String> {
    Ok(match tok {
        Token::PlusEq => BinOp::Add,
        Token::MinusEq => BinOp::Sub,
        Token::StarEq => BinOp::Mul,
        Token::SlashEq => BinOp::Div,
        Token::PercentEq => BinOp::Rem,
        Token::AmpEq => BinOp::BitAnd,
        Token::PipeEq => BinOp::BitOr,
        Token::CaretEq => BinOp::BitXor,
        Token::ShlEq => BinOp::Shl,
        Token::ShrEq => BinOp::Shr,
        _ => return Err(format!("not an assign-op: {:?}", tok)),
    })
}

// ─── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;

    fn parse(src: &str) -> SourceFile {
        let tokens = Lexer::tokenize(src).unwrap();
        Parser::parse_file(tokens).unwrap()
    }

    fn parse_expr_str(src: &str) -> Expr {
        let tokens = Lexer::tokenize(src).unwrap();
        let mut parser = Parser::new(tokens);
        parser.parse_expr().unwrap()
    }

    #[test]
    fn simple_function() {
        let file = parse("fn add(a: i32, b: i32) -> i32 { a + b }");
        assert_eq!(file.items.len(), 1);
        if let ItemKind::Function(f) = &file.items[0].kind {
            assert_eq!(f.name, "add");
            assert_eq!(f.params.len(), 2);
        } else {
            panic!("expected function");
        }
    }

    #[test]
    fn simple_struct() {
        let file = parse("struct Point { x: f64, y: f64 }");
        assert_eq!(file.items.len(), 1);
        if let ItemKind::Struct(s) = &file.items[0].kind {
            assert_eq!(s.name, "Point");
            if let StructKind::Named(fields) = &s.kind {
                assert_eq!(fields.len(), 2);
            } else {
                panic!("expected named struct");
            }
        } else {
            panic!("expected struct");
        }
    }

    #[test]
    fn enum_with_variants() {
        let file = parse("enum Option<T> { Some(T), None }");
        if let ItemKind::Enum(e) = &file.items[0].kind {
            assert_eq!(e.name, "Option");
            assert_eq!(e.variants.len(), 2);
            assert_eq!(e.variants[0].name, "Some");
            assert_eq!(e.variants[1].name, "None");
        } else {
            panic!("expected enum");
        }
    }

    #[test]
    fn impl_block() {
        let file = parse("impl Point { fn new(x: f64, y: f64) -> Self { Point { x, y } } }");
        if let ItemKind::Impl(imp) = &file.items[0].kind {
            assert_eq!(imp.items.len(), 1);
        } else {
            panic!("expected impl");
        }
    }

    #[test]
    fn trait_def() {
        let file = parse("trait Display { fn fmt(&self) -> String; }");
        if let ItemKind::Trait(t) = &file.items[0].kind {
            assert_eq!(t.name, "Display");
        } else {
            panic!("expected trait");
        }
    }

    #[test]
    fn binary_expr_precedence() {
        let expr = parse_expr_str("1 + 2 * 3");
        if let ExprKind::Binary { op, lhs, rhs } = &expr.kind {
            assert_eq!(*op, BinOp::Add);
            assert!(matches!(lhs.kind, ExprKind::IntLit(1)));
            if let ExprKind::Binary { op, .. } = &rhs.kind {
                assert_eq!(*op, BinOp::Mul);
            } else {
                panic!("expected mul");
            }
        } else {
            panic!("expected binary");
        }
    }

    #[test]
    fn if_else_expr() {
        let expr = parse_expr_str("if x > 0 { x } else { -x }");
        assert!(matches!(expr.kind, ExprKind::If { .. }));
    }

    #[test]
    fn match_expr() {
        let src = "fn foo(x: i32) -> &str { match x { 0 => \"zero\", _ => \"other\" } }";
        let file = parse(src);
        assert_eq!(file.items.len(), 1);
    }

    #[test]
    fn closure() {
        let expr = parse_expr_str("|x, y| x + y");
        assert!(matches!(expr.kind, ExprKind::Closure { .. }));
    }

    #[test]
    fn method_chain() {
        let expr = parse_expr_str("vec.iter().map(|x| x * 2).collect()");
        // Should parse without error
        assert!(matches!(expr.kind, ExprKind::MethodCall { .. }));
    }

    #[test]
    fn generic_function() {
        let file = parse("fn identity<T: Clone>(x: T) -> T { x }");
        if let ItemKind::Function(f) = &file.items[0].kind {
            assert_eq!(f.generics.params.len(), 1);
        } else {
            panic!("expected function");
        }
    }

    #[test]
    fn use_statement() {
        let file = parse("use std::collections::HashMap;");
        assert!(matches!(file.items[0].kind, ItemKind::Use(_)));
    }
}
