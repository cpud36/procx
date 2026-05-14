use proc_macro::{Delimiter, Group, Ident, Literal, Punct, Spacing, Span, TokenStream, TokenTree};

type InputIter = std::iter::Peekable<proc_macro::token_stream::IntoIter>;

#[proc_macro]
pub fn cmd(input: TokenStream) -> TokenStream {
    try_macro(input, |input, errors| {
        let mut input = input.into_iter().peekable();
        let krate = Krate::new(&mut input);
        let args = lex(input, &krate, errors)?;
        let mut args = args.into_iter();
        let cmd = args.next().ok_or_else(|| {
            errors.emit(
                "expected the command to be specified",
                proc_macro::Span::call_site(),
            )
        })?;
        let mut acc = TokenStream::new();
        if args.len() == 0 {
            krate.new_cmd(cmd.into_tt(&krate), &mut acc);
            return Ok(acc);
        }
        let stok = let_mut("acc", &mut acc, |acc| {
            krate.new_cmd(cmd.into_tt(&krate), acc)
        });
        extend_args(args, &stok, &mut acc, &krate);
        acc.extend([stok]);
        let tt = TokenTree::Group(Group::new(Delimiter::Brace, acc));
        Ok(TokenStream::from_iter([tt]))
    })
}

#[proc_macro]
pub fn arg(input: TokenStream) -> TokenStream {
    try_macro(input, |input, errors| {
        let mut input = input.into_iter().peekable();
        let krate = Krate::new(&mut input);
        let args = lex(input, &krate, errors)?;
        let mut args = args.into_iter();
        let arg = args
            .next()
            .ok_or_else(|| errors.emit("expected the argument", proc_macro::Span::call_site()))?;
        if let Some(second) = args.next() {
            errors.emit("expected at most one argument", second.span());
        }
        if let CmdPart::Splat(_) = &arg {
            return Err(errors.emit(
                "splat arguments are not yet supported, use args! instead",
                arg.span(),
            ));
        }
        let mut acc = TokenStream::new();
        krate.new_arg(arg.into_tt(&krate), &mut acc);
        Ok(acc)
    })
}

#[proc_macro]
pub fn args(input: TokenStream) -> TokenStream {
    try_macro(input, |input, errors| {
        let mut input = input.into_iter().peekable();
        let krate = Krate::new(&mut input);
        let args = lex(input, &krate, errors)?;
        let mut acc = TokenStream::new();
        if args.is_empty() {
            krate.new_args(&mut acc);
            return Ok(acc);
        }
        let stok = let_mut("acc", &mut acc, |acc| krate.new_args(acc));
        extend_args(args.into_iter(), &stok, &mut acc, &krate);
        acc.extend([stok]);
        let tt = TokenTree::Group(Group::new(Delimiter::Brace, acc));
        Ok(TokenStream::from_iter([tt]))
    })
}

fn extend_args(
    args: std::vec::IntoIter<CmdPart>,
    target: &TokenTree,
    acc: &mut TokenStream,
    krate: &Krate,
) {
    for arg in args {
        match arg {
            CmdPart::Arg(_) | CmdPart::Combined(_) => {
                acc.extend([target.clone()]);
                krate.arg_method(arg.into_tt(&krate), acc);
                acc.extend([TokenTree::Punct(Punct::new(';', Spacing::Alone))]);
            }
            CmdPart::Splat(_) => {
                acc.extend([target.clone()]);
                krate.args_method(arg.into_tt(&krate), acc);
                acc.extend([TokenTree::Punct(Punct::new(';', Spacing::Alone))]);
            }
        }
    }
}

fn lex(
    mut input: InputIter,
    krate: &Krate,
    errors: &mut Errors,
) -> Result<Vec<CmdPart>, ErrorGuaranteed> {
    let cmd = input
        .next()
        .ok_or_else(|| errors.emit("expected command", krate.full_span))?;
    let (cmd, cmd_span) = string_literal(&cmd, errors)?;
    let mut cmd = TextCursor::new(cmd.as_str(), cmd_span);
    let mut args = Vec::new();
    let mut prev_splat: Option<Token> = None;
    while let Ok(Some(token)) = cmd.eat_token(errors) {
        if let Some(splat) = prev_splat.take() {
            if token.joined_to_prev {
                errors.emit(
                    &format!(
                        "can't combine splat with concatentaion, add spaces around {{{}..}}``",
                        splat.text
                    ),
                    splat.span,
                );
            }
        }
        let tt = match token.kind {
            TokenKind::Word | TokenKind::String => token.to_tt(&krate),
            TokenKind::Interpolation { inline: false, .. } => {
                errors.emit(
                    "non-inline interpolation arguments are not yet supported",
                    token.span,
                );
                continue;
            }
            TokenKind::Interpolation { splat: false, .. } => {
                let tok = token.to_tt(&krate);
                let and_amp = TokenStream::from_iter([
                    TokenTree::Punct(Punct::new('&', Spacing::Alone)),
                    tok,
                ]);
                TokenTree::Group(Group::new(Delimiter::Parenthesis, and_amp))
            }
            TokenKind::Interpolation { splat: true, .. } => {
                prev_splat = Some(token.clone());
                args.push(CmdPart::Splat(token.to_tt(&krate)));
                continue;
            }
        };
        match (token.joined_to_prev, args.last_mut()) {
            (true, Some(arg)) => arg.extend_word(tt),
            _ => args.push(CmdPart::Arg(tt)),
        }
    }

    if let Some(tok) = input.next() {
        errors.emit("interpolation arguments are not yet supported", tok.span());
    }

    Ok(args)
}

struct Krate {
    full_span: Span,
    private_span: Span,
    plumbing: TokenStream,
}

impl Krate {
    fn new(input: &mut InputIter) -> Self {
        let opts = input.peek().and_then(|tt| match tt {
            TokenTree::Group(group) if group.delimiter() == Delimiter::Parenthesis => {
                Some(group.stream())
            }
            _ => None,
        });
        if opts.is_some() {
            input.next();
        }
        let plumbing = match opts {
            Some(opts) => opts,
            None => {
                let span = proc_macro::Span::mixed_site();
                TokenStream::from_iter([
                    TokenTree::Punct(Punct::new(':', Spacing::Joint)),
                    TokenTree::Punct(Punct::new(':', Spacing::Alone)),
                    TokenTree::Ident(Ident::new("xproc", span)),
                    TokenTree::Punct(Punct::new(':', Spacing::Joint)),
                    TokenTree::Punct(Punct::new(':', Spacing::Alone)),
                    TokenTree::Ident(Ident::new("plumbing", span)),
                ])
            }
        };

        let call_site = proc_macro::Span::call_site();
        Self {
            full_span: call_site,
            private_span: proc_macro::Span::mixed_site(),
            plumbing,
        }
    }

    fn plumbing(&self, ident: &str, acc: &mut TokenStream) {
        let span = self.private_span;
        acc.extend(self.plumbing.clone().into_iter());
        acc.extend([
            TokenTree::Punct(Punct::new(':', Spacing::Joint)),
            TokenTree::Punct(Punct::new(':', Spacing::Alone)),
            TokenTree::Ident(Ident::new(ident, span)),
        ]);
    }

    fn call(&self, ident: &str, args: TokenStream, acc: &mut TokenStream) {
        self.plumbing(ident, acc);
        acc.extend([TokenTree::Group(Group::new(Delimiter::Parenthesis, args))]);
    }

    fn method(&self, ident: &str, args: TokenStream, acc: &mut TokenStream) {
        let span = self.private_span;
        acc.extend([
            TokenTree::Punct(Punct::new('.', Spacing::Alone)),
            TokenTree::Ident(Ident::new(ident, span)),
            TokenTree::Group(Group::new(Delimiter::Parenthesis, args)),
        ]);
    }

    fn new_string(&self, acc: &mut TokenStream) {
        self.call("new_string", TokenStream::new(), acc);
    }

    fn new_cmd(&self, cmd: TokenTree, acc: &mut TokenStream) {
        self.call("new_cmd", TokenStream::from_iter([cmd]), acc);
    }

    fn new_arg(&self, arg: TokenTree, acc: &mut TokenStream) {
        self.call("new_arg", TokenStream::from_iter([arg]), acc);
    }

    fn new_args(&self, acc: &mut TokenStream) {
        self.call("new_args", TokenStream::new(), acc);
    }

    fn arg_method(&self, arg: TokenTree, acc: &mut TokenStream) {
        self.method("arg", TokenStream::from_iter([arg]), acc);
    }

    fn args_method(&self, args: TokenTree, acc: &mut TokenStream) {
        self.method("args", TokenStream::from_iter([args]), acc);
    }
}

enum CmdPart {
    Arg(TokenTree),
    Combined(Vec<TokenTree>),
    Splat(TokenTree),
}

impl CmdPart {
    fn span(&self) -> Span {
        match self {
            Self::Arg(tt) => tt.span(),
            Self::Combined(parts) => parts.iter().map(|tt| tt.span()).next().unwrap(),
            Self::Splat(tt) => tt.span(),
        }
    }

    fn into_tt(self, krate: &Krate) -> TokenTree {
        match self {
            Self::Arg(tt) => tt,
            Self::Combined(parts) => concat_os_string(parts, krate),
            Self::Splat(tt) => tt,
        }
    }

    fn extend_word(&mut self, tt: TokenTree) {
        match self {
            Self::Arg(arg) => {
                *self = Self::Combined(vec![arg.clone(), tt]);
            }
            Self::Combined(parts) => {
                parts.push(tt);
            }
            Self::Splat(_splat) => {
                // do nothing
            }
        }
    }
}

fn concat_os_string(parts: Vec<TokenTree>, krate: &Krate) -> TokenTree {
    let span = proc_macro::Span::mixed_site();
    let mut acc = TokenStream::new();
    let stok = let_mut("acc", &mut acc, |acc| krate.new_string(acc));
    for part in parts {
        let push_args = TokenStream::from_iter([part]);
        acc.extend([
            stok.clone(),
            TokenTree::Punct(Punct::new('.', Spacing::Alone)),
            TokenTree::Ident(Ident::new("push", span)),
            TokenTree::Group(Group::new(Delimiter::Parenthesis, push_args)),
            TokenTree::Punct(Punct::new(';', Spacing::Alone)),
        ]);
    }
    acc.extend([stok.clone()]);
    TokenTree::Group(Group::new(Delimiter::Brace, acc))
}

fn let_mut(ident: &str, acc: &mut TokenStream, init: impl FnOnce(&mut TokenStream)) -> TokenTree {
    let span = proc_macro::Span::mixed_site();
    let stok = TokenTree::Ident(Ident::new(ident, span));
    acc.extend([
        TokenTree::Ident(Ident::new("let", span)),
        TokenTree::Ident(Ident::new("mut", span)),
        stok.clone(),
        TokenTree::Punct(Punct::new('=', Spacing::Alone)),
    ]);
    init(acc);
    acc.extend([TokenTree::Punct(Punct::new(';', Spacing::Alone))]);
    stok
}

#[derive(Clone, Debug)]
struct Token<'a> {
    joined_to_prev: bool,
    kind: TokenKind,
    text: &'a str,
    span: Span,
}

#[derive(Clone, Debug)]
enum TokenKind {
    Word,
    String,
    Interpolation { splat: bool, inline: bool },
}

impl<'a> Token<'a> {
    fn to_tt(&self, krate: &Krate) -> TokenTree {
        match self.kind {
            TokenKind::Word => TokenTree::Literal(Literal::string(self.text)),
            TokenKind::String => TokenTree::Literal(Literal::string(self.text)),
            TokenKind::Interpolation { inline, .. } => {
                let span = if inline {
                    self.span
                } else {
                    krate.private_span
                };
                let ident = Ident::new(self.text, span);
                TokenTree::Ident(ident)
            }
        }
    }
}

struct TextCursor<'a> {
    left: &'a str,
    span: Span,
}

impl<'a> TextCursor<'a> {
    fn new(text: &'a str, span: Span) -> Self {
        Self { left: text, span }
    }

    fn eat_spaces(&mut self) -> bool {
        let len = self.left.len();
        self.left = self.left.trim_start();
        len != self.left.len()
    }

    fn eat_token(&mut self, errors: &mut Errors) -> Result<Option<Token<'a>>, ErrorGuaranteed> {
        let joined_to_prev = !self.eat_spaces();
        let text = self.left;
        if text.is_empty() {
            return Ok(None);
        }
        let span = self.span;

        if let Some(text) = text.strip_prefix('{') {
            let len = text
                .find('}')
                .map(|it| it + 1)
                .ok_or_else(|| self.emit_error(errors, "unclosed `{` in command"))?;
            let (part, rest) = text.split_at(len);
            let mut part = part.strip_suffix('}').unwrap();
            let mut splat = false;
            if let Some(p) = part.strip_suffix("..") {
                splat = true;
                part = p;
            }
            let inline = !part.is_empty();
            self.left = rest;
            let token = Token {
                joined_to_prev,
                kind: TokenKind::Interpolation { splat, inline },
                text: part,
                span,
            };
            return Ok(Some(token));
        }

        if let Some(text) = text.strip_prefix('\'') {
            let len = text
                .find('\'')
                .map(|it| it + 1)
                .ok_or_else(|| self.emit_error(errors, "unclosed `'` in command"))?;
            let (part, rest) = text.split_at(len);
            let part = part.strip_suffix('\'').unwrap();
            self.left = rest;
            let token = Token {
                joined_to_prev,
                kind: TokenKind::String,
                text: part,
                span,
            };
            return Ok(Some(token));
        }

        let len = text
            .find(|it: char| it.is_ascii_whitespace() || it == '\'' || it == '{')
            .unwrap_or(text.len());
        let (part, rest) = text.split_at(len);
        let token = Token {
            joined_to_prev,
            kind: TokenKind::Word,
            text: part,
            span,
        };
        self.left = rest;
        Ok(Some(token))
    }

    fn emit_error(&mut self, errors: &mut Errors, message: &str) -> ErrorGuaranteed {
        self.left = "";
        errors.emit(message, self.span)
    }
}

fn string_literal(
    input: &TokenTree,
    errors: &mut Errors,
) -> Result<(String, Span), ErrorGuaranteed> {
    let literal = match input {
        TokenTree::Literal(literal) => Some(literal.clone()),
        TokenTree::Group(g) => match g.delimiter() {
            Delimiter::None => {
                let mut iter = g.stream().into_iter();
                match (iter.next(), iter.next()) {
                    (Some(TokenTree::Literal(literal)), None) => Some(literal),
                    _ => None,
                }
            }
            _ => None,
        },
        _ => None,
    };
    let literal = literal.ok_or_else(|| errors.emit("expected string literal", input.span()))?;
    let span = literal.span();

    let text = literal.to_string();
    if let Some(text) = trim_quotes(&text, '\'') {
        return Ok((text, span));
    }
    if let Some(text) = trim_quotes(&text, '"') {
        return Ok((text, span));
    }

    Err(errors.emit("expected string literal", span))
}

fn trim_quotes(text: &str, quote: char) -> Option<String> {
    let text = text.strip_prefix(quote)?.strip_suffix(quote)?;
    // TODO: handle escapes?
    Some(text.to_string())
}

#[derive(Clone, Copy)]
struct ErrorGuaranteed(());

fn try_macro(
    input: TokenStream,
    f: impl FnOnce(TokenStream, &mut Errors) -> Result<TokenStream, ErrorGuaranteed>,
) -> TokenStream {
    let mut errors = Errors::default();
    match f(input, &mut errors) {
        Ok(mut output) => {
            errors.append_to(&mut output);
            output
        }
        Err(guar) => errors.to_error(guar),
    }
}

#[derive(Default)]
struct Errors {
    errors: Vec<Error>,
}

impl Errors {
    fn emit(&mut self, message: &str, span: Span) -> ErrorGuaranteed {
        self.errors.push(Error {
            message: message.to_string(),
            span,
        });
        ErrorGuaranteed(())
    }

    fn to_error(&self, guar: ErrorGuaranteed) -> TokenStream {
        let _ = guar;
        let mut acc = TokenStream::new();
        self.append_to(&mut acc);
        if self.errors.is_empty() {
            panic!("expected at least one error");
        }
        acc
    }

    fn append_to(&self, acc: &mut TokenStream) {
        for error in &self.errors {
            error.append_to(acc);
        }
    }
}

struct Error {
    message: String,
    span: Span,
}

impl Error {
    fn append_to(&self, acc: &mut TokenStream) {
        let span = self.span;
        acc.extend([
            TokenTree::Ident(Ident::new("compile_error", span)),
            TokenTree::Punct(Punct::new('!', Spacing::Alone)),
            TokenTree::Group(Group::new(
                Delimiter::Parenthesis,
                TokenStream::from_iter([TokenTree::Literal(Literal::string(
                    self.message.as_str(),
                ))]),
            )),
            TokenTree::Punct(Punct::new(';', Spacing::Alone)),
        ]);
    }
}
