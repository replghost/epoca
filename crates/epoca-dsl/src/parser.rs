use crate::ast::*;

/// A parse error with location and context.
#[derive(Debug, Clone)]
pub struct ParseError {
    pub line: usize,
    pub col: usize,
    pub message: String,
    pub source_line: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}:{}: {}", self.line, self.col, self.message)?;
        if !self.source_line.is_empty() {
            write!(f, "\n  | {}", self.source_line)?;
            write!(f, "\n  | {}^", " ".repeat(self.col.saturating_sub(1)))?;
        }
        Ok(())
    }
}

impl std::error::Error for ParseError {}

type Result<T> = std::result::Result<T, ParseError>;

/// Known element kinds that map to NodeKind.
const ELEMENT_KINDS: &[&str] = &[
    "VStack", "HStack", "ZStack", "Text", "Button", "Input", "List", "Image", "Table", "Chart",
    "Spacer", "Divider", "Container",
];

/// Hand-written recursive descent parser for ZML.
pub struct Parser<'a> {
    #[allow(dead_code)]
    src: &'a str,
    chars: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
    lines: Vec<&'a str>,
}

impl<'a> Parser<'a> {
    pub fn new(src: &'a str) -> Self {
        let chars: Vec<char> = src.chars().collect();
        let lines: Vec<&str> = src.lines().collect();
        Self {
            src,
            chars,
            pos: 0,
            line: 1,
            col: 1,
            lines,
        }
    }

    /// Parse a complete ZML app.
    pub fn parse_app(&mut self) -> Result<ZmlApp> {
        let mut permissions = None;
        let mut state_block = Vec::new();
        let mut body = Vec::new();

        self.skip_blank_lines();

        while !self.at_end() {
            self.skip_blank_lines();
            if self.at_end() {
                break;
            }

            let indent = self.current_indent();
            if indent > 0 {
                return Err(self.error("top-level items must start at column 1"));
            }

            let word = self.peek_word();
            match word.as_str() {
                "permissions" => {
                    if permissions.is_some() {
                        return Err(self.error("duplicate 'permissions' block"));
                    }
                    permissions = Some(self.parse_permissions_block()?);
                }
                "state" => {
                    if !state_block.is_empty() {
                        return Err(self.error("duplicate 'state' block"));
                    }
                    state_block = self.parse_state_block()?;
                }
                "--" => {
                    self.skip_line();
                }
                w if ELEMENT_KINDS.contains(&w) => {
                    body.push(self.parse_element(0)?);
                }
                _ => {
                    return Err(self.error_msg(format!(
                        "unexpected '{}' — expected 'permissions', 'state', or an element like VStack, Text, Button",
                        word
                    )));
                }
            }
        }

        Ok(ZmlApp {
            permissions,
            state_block,
            body,
        })
    }

    // -----------------------------------------------------------------------
    // Permissions block
    // -----------------------------------------------------------------------

    fn parse_permissions_block(&mut self) -> Result<ZmlPermissions> {
        self.expect_word("permissions")?;
        self.skip_to_eol();
        self.advance_line();

        let mut perms = ZmlPermissions::default();
        let block_indent = self.current_indent();
        if block_indent == 0 {
            return Ok(perms);
        }

        while !self.at_end() {
            let indent = self.current_indent();
            if indent < block_indent {
                break;
            }
            if self.is_blank_line() {
                self.advance_line();
                continue;
            }
            if self.is_comment_line() {
                self.skip_line();
                continue;
            }

            self.skip_spaces();
            let key = self.read_word();
            self.skip_spaces();
            self.expect_char('=')?;
            self.skip_spaces();

            match key.as_str() {
                "network" => {
                    perms.network = self.parse_string_list()?;
                }
                "storage" => {
                    perms.storage = Some(self.parse_quoted_string()?);
                }
                "camera" => {
                    perms.camera = self.parse_bool_value()?;
                }
                "geolocation" => {
                    perms.geolocation = self.parse_quoted_string()?;
                }
                "gpu" => {
                    perms.gpu = self.parse_quoted_string()?;
                }
                _ => {
                    return Err(self.error_msg(format!(
                        "unknown permission key '{key}' — expected network, storage, camera, geolocation, or gpu"
                    )));
                }
            }
            self.skip_to_eol();
            self.advance_line();
        }

        Ok(perms)
    }

    fn parse_string_list(&mut self) -> Result<Vec<String>> {
        self.expect_char('[')?;
        self.skip_spaces();
        let mut items = Vec::new();
        if self.peek_char() == Some(']') {
            self.advance();
            return Ok(items);
        }
        loop {
            self.skip_spaces();
            items.push(self.parse_quoted_string()?);
            self.skip_spaces();
            if self.peek_char() == Some(']') {
                self.advance();
                break;
            }
            self.expect_char(',')?;
        }
        Ok(items)
    }

    fn parse_bool_value(&mut self) -> Result<bool> {
        let word = self.read_word();
        match word.as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(self.error_msg(format!("expected 'true' or 'false', found '{word}'"))),
        }
    }

    // -----------------------------------------------------------------------
    // State block
    // -----------------------------------------------------------------------

    fn parse_state_block(&mut self) -> Result<Vec<(String, Expr)>> {
        self.expect_word("state")?;
        self.skip_to_eol();
        self.advance_line();

        let mut bindings = Vec::new();
        let block_indent = self.current_indent();
        if block_indent == 0 {
            return Ok(bindings);
        }

        while !self.at_end() {
            let indent = self.current_indent();
            if indent < block_indent {
                break;
            }
            if self.is_blank_line() {
                self.advance_line();
                continue;
            }
            if self.is_comment_line() {
                self.skip_line();
                continue;
            }

            self.skip_spaces();
            let name = self.read_word();
            if name.is_empty() {
                return Err(self.error("expected variable name in state block"));
            }
            self.skip_spaces();
            self.expect_char('=')?;
            self.skip_spaces();
            let value = self.parse_expr()?;
            bindings.push((name, value));
            self.skip_to_eol();
            self.advance_line();
        }

        Ok(bindings)
    }

    // -----------------------------------------------------------------------
    // Element parsing
    // -----------------------------------------------------------------------

    fn parse_element(&mut self, _parent_indent: usize) -> Result<Node> {
        let el_indent = self.current_indent();
        self.skip_spaces();

        let kind = self.read_word();
        if !ELEMENT_KINDS.contains(&kind.as_str()) {
            return Err(self.error_msg(format!(
                "unknown element '{kind}' — expected one of: {}",
                ELEMENT_KINDS.join(", ")
            )));
        }

        // Parse inline props and optional inline text content
        let mut props = Vec::new();
        let mut inline_text: Option<Expr> = None;
        self.skip_spaces();

        // Check for inline quoted string (e.g., Text "hello" or Button "+")
        if self.peek_char() == Some('"') {
            inline_text = Some(self.parse_string_expr()?);
            self.skip_spaces();
        }

        // Parse key=value props
        while !self.at_eol() && self.peek_char() != Some('-') {
            let save_pos = self.pos;
            let save_line = self.line;
            let save_col = self.col;

            let key = self.read_word();
            if key.is_empty() {
                break;
            }
            if self.peek_char() != Some('=') {
                // Not a prop, backtrack
                self.pos = save_pos;
                self.line = save_line;
                self.col = save_col;
                break;
            }
            self.advance(); // consume '='
            let value = self.parse_prop_value()?;
            props.push(Prop { key, value });
            self.skip_spaces();
        }

        // Add inline text as "content" or "label" prop depending on element kind
        if let Some(text_expr) = inline_text {
            let prop_name = match kind.as_str() {
                "Button" => "label",
                _ => "content",
            };
            props.insert(0, Prop {
                key: prop_name.to_string(),
                value: text_expr,
            });
        }

        self.skip_to_eol();
        self.advance_line();

        // Parse children and handlers
        let child_indent = el_indent + 2;
        let mut children = Vec::new();
        let mut handlers = Vec::new();

        while !self.at_end() {
            if self.is_blank_line() {
                self.advance_line();
                continue;
            }
            if self.is_comment_line() {
                self.skip_line();
                continue;
            }

            let indent = self.current_indent();
            if indent < child_indent {
                break;
            }

            // Peek at what's at this indent level
            let word = self.peek_word_at(indent);
            if word == "on" {
                handlers.push(self.parse_handler(child_indent)?);
            } else if ELEMENT_KINDS.contains(&word.as_str()) {
                children.push(self.parse_element(child_indent)?);
            } else {
                return Err(self.error_msg(format!(
                    "unexpected '{word}' inside {kind} — expected an element (VStack, Text, Button, ...) or 'on' handler"
                )));
            }
        }

        Ok(Node::Element {
            kind,
            props,
            children,
            handlers,
        })
    }

    // -----------------------------------------------------------------------
    // Handler parsing
    // -----------------------------------------------------------------------

    fn parse_handler(&mut self, _parent_indent: usize) -> Result<Handler> {
        let handler_indent = self.current_indent();
        self.skip_spaces();
        self.expect_word("on")?;
        self.skip_spaces();
        let event = self.read_word();
        if event.is_empty() {
            return Err(self.error("expected event name after 'on' (e.g., 'click', 'input', 'submit')"));
        }
        self.skip_to_eol();
        self.advance_line();

        let action_indent = handler_indent + 2;
        let mut actions = Vec::new();

        while !self.at_end() {
            if self.is_blank_line() {
                self.advance_line();
                continue;
            }
            if self.is_comment_line() {
                self.skip_line();
                continue;
            }

            let indent = self.current_indent();
            if indent < action_indent {
                break;
            }

            actions.push(self.parse_action()?);
            self.skip_to_eol();
            self.advance_line();
        }

        Ok(Handler { event, actions })
    }

    fn parse_action(&mut self) -> Result<Action> {
        self.skip_spaces();
        let name = self.read_word();
        if name.is_empty() {
            return Err(self.error("expected variable name in action"));
        }

        // Check for dotted path
        let mut path = vec![name];
        while self.peek_char() == Some('.') {
            self.advance();
            let seg = self.read_word();
            if seg.is_empty() {
                return Err(self.error("expected path segment after '.'"));
            }
            path.push(seg);
        }

        self.skip_spaces();
        self.expect_char('=')?;
        self.skip_spaces();
        let value = self.parse_expr()?;

        Ok(Action::Set { path, value })
    }

    // -----------------------------------------------------------------------
    // Expression parsing (Pratt / precedence climbing)
    // -----------------------------------------------------------------------

    fn parse_expr(&mut self) -> Result<Expr> {
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Result<Expr> {
        let mut left = self.parse_additive()?;
        loop {
            self.skip_spaces();
            let op = if self.try_consume("==") {
                BinOp::Eq
            } else if self.try_consume("!=") {
                BinOp::Ne
            } else if self.try_consume("<=") {
                BinOp::Le
            } else if self.try_consume(">=") {
                BinOp::Ge
            } else if self.try_consume("<") {
                BinOp::Lt
            } else if self.try_consume(">") {
                BinOp::Gt
            } else {
                break;
            };
            self.skip_spaces();
            let right = self.parse_additive()?;
            left = Expr::BinOp(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<Expr> {
        let mut left = self.parse_multiplicative()?;
        loop {
            self.skip_spaces();
            let op = if self.peek_char() == Some('+') {
                self.advance();
                BinOp::Add
            } else if self.peek_char() == Some('-') && !self.at_eol_after(1) {
                self.advance();
                BinOp::Sub
            } else {
                break;
            };
            self.skip_spaces();
            let right = self.parse_multiplicative()?;
            left = Expr::BinOp(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr> {
        let mut left = self.parse_unary()?;
        loop {
            self.skip_spaces();
            let op = if self.peek_char() == Some('*') {
                self.advance();
                BinOp::Mul
            } else if self.peek_char() == Some('/') {
                self.advance();
                BinOp::Div
            } else if self.peek_char() == Some('%') {
                self.advance();
                BinOp::Mod
            } else {
                break;
            };
            self.skip_spaces();
            let right = self.parse_unary()?;
            left = Expr::BinOp(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr> {
        if self.peek_char() == Some('-') {
            self.advance();
            let expr = self.parse_primary()?;
            return Ok(Expr::Negate(Box::new(expr)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        self.skip_spaces();

        match self.peek_char() {
            Some('"') => self.parse_string_expr(),
            Some('(') => {
                self.advance(); // consume '('
                self.skip_spaces();
                let expr = self.parse_expr()?;
                self.skip_spaces();
                self.expect_char(')')?;
                Ok(expr)
            }
            Some(c) if c.is_ascii_digit() => self.parse_number(),
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                let word = self.read_word();
                match word.as_str() {
                    "true" => Ok(Expr::Literal(ZmlValue::Bool(true))),
                    "false" => Ok(Expr::Literal(ZmlValue::Bool(false))),
                    "null" => Ok(Expr::Literal(ZmlValue::Null)),
                    _ => {
                        // Path: word(.word)*
                        let mut path = vec![word];
                        while self.peek_char() == Some('.') {
                            self.advance();
                            let seg = self.read_word();
                            if seg.is_empty() {
                                return Err(self.error("expected path segment after '.'"));
                            }
                            path.push(seg);
                        }
                        Ok(Expr::Path(path))
                    }
                }
            }
            Some(c) => Err(self.error_msg(format!("unexpected character '{c}' in expression"))),
            None => Err(self.error("unexpected end of input in expression")),
        }
    }

    fn parse_number(&mut self) -> Result<Expr> {
        let start = self.pos;
        let mut is_float = false;

        while let Some(c) = self.peek_char() {
            if c.is_ascii_digit() {
                self.advance();
            } else if c == '.' && !is_float {
                // Check next char is a digit (not a method call)
                if self.pos + 1 < self.chars.len() && self.chars[self.pos + 1].is_ascii_digit() {
                    is_float = true;
                    self.advance();
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        let num_str: String = self.chars[start..self.pos].iter().collect();
        if is_float {
            let f: f64 = num_str.parse().map_err(|_| self.error("invalid number"))?;
            Ok(Expr::Literal(ZmlValue::Float(f)))
        } else {
            let n: i64 = num_str
                .parse()
                .map_err(|_| self.error("integer too large"))?;
            Ok(Expr::Literal(ZmlValue::Int(n)))
        }
    }

    fn parse_string_expr(&mut self) -> Result<Expr> {
        self.expect_char('"')?;
        let mut parts: Vec<InterpolPart> = Vec::new();
        let mut current_lit = String::new();

        loop {
            match self.peek_char() {
                None => return Err(self.error("unterminated string — missing closing '\"'")),
                Some('"') => {
                    self.advance();
                    break;
                }
                Some('{') => {
                    self.advance();
                    if !current_lit.is_empty() {
                        parts.push(InterpolPart::Literal(std::mem::take(&mut current_lit)));
                    }
                    let expr = self.parse_expr()?;
                    self.skip_spaces();
                    self.expect_char('}')?;
                    parts.push(InterpolPart::Expr(expr));
                }
                Some('\\') => {
                    self.advance();
                    match self.peek_char() {
                        Some('n') => {
                            self.advance();
                            current_lit.push('\n');
                        }
                        Some('t') => {
                            self.advance();
                            current_lit.push('\t');
                        }
                        Some('"') => {
                            self.advance();
                            current_lit.push('"');
                        }
                        Some('\\') => {
                            self.advance();
                            current_lit.push('\\');
                        }
                        Some('{') => {
                            self.advance();
                            current_lit.push('{');
                        }
                        Some(c) => {
                            current_lit.push('\\');
                            current_lit.push(c);
                            self.advance();
                        }
                        None => return Err(self.error("unterminated escape sequence")),
                    }
                }
                Some(c) => {
                    current_lit.push(c);
                    self.advance();
                }
            }
        }

        if !current_lit.is_empty() {
            parts.push(InterpolPart::Literal(current_lit));
        }

        // Optimize: if it's a single literal part with no interpolation, return Str
        if parts.len() == 1 {
            if let InterpolPart::Literal(s) = &parts[0] {
                return Ok(Expr::Literal(ZmlValue::Str(s.clone())));
            }
        }
        if parts.is_empty() {
            return Ok(Expr::Literal(ZmlValue::Str(String::new())));
        }

        Ok(Expr::Interpolated(parts))
    }

    fn parse_prop_value(&mut self) -> Result<Expr> {
        match self.peek_char() {
            Some('"') => self.parse_string_expr(),
            Some(c) if c.is_ascii_digit() => self.parse_number(),
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                let word = self.read_word();
                match word.as_str() {
                    "true" => Ok(Expr::Literal(ZmlValue::Bool(true))),
                    "false" => Ok(Expr::Literal(ZmlValue::Bool(false))),
                    _ => Ok(Expr::Literal(ZmlValue::Str(word))),
                }
            }
            _ => Err(self.error("expected a value (string, number, or identifier)")),
        }
    }

    fn parse_quoted_string(&mut self) -> Result<String> {
        self.expect_char('"')?;
        let mut s = String::new();
        loop {
            match self.peek_char() {
                None => return Err(self.error("unterminated string")),
                Some('"') => {
                    self.advance();
                    return Ok(s);
                }
                Some('\\') => {
                    self.advance();
                    match self.peek_char() {
                        Some(c) => {
                            s.push(c);
                            self.advance();
                        }
                        None => return Err(self.error("unterminated escape")),
                    }
                }
                Some(c) => {
                    s.push(c);
                    self.advance();
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Low-level helpers
    // -----------------------------------------------------------------------

    fn at_end(&self) -> bool {
        self.pos >= self.chars.len()
    }

    fn peek_char(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn advance(&mut self) {
        if self.pos < self.chars.len() {
            if self.chars[self.pos] == '\n' {
                self.line += 1;
                self.col = 1;
            } else {
                self.col += 1;
            }
            self.pos += 1;
        }
    }

    fn skip_spaces(&mut self) {
        while let Some(c) = self.peek_char() {
            if c == ' ' || c == '\t' {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn skip_to_eol(&mut self) {
        // Skip any trailing comment
        while let Some(c) = self.peek_char() {
            if c == '\n' {
                break;
            }
            self.advance();
        }
    }

    fn advance_line(&mut self) {
        if self.peek_char() == Some('\n') {
            self.advance();
        }
    }

    fn skip_line(&mut self) {
        self.skip_to_eol();
        self.advance_line();
    }

    fn skip_blank_lines(&mut self) {
        while !self.at_end() {
            if self.is_blank_line() {
                self.advance_line();
            } else if self.is_comment_line() {
                self.skip_line();
            } else {
                break;
            }
        }
    }

    fn is_blank_line(&self) -> bool {
        let mut p = self.pos;
        while p < self.chars.len() {
            let c = self.chars[p];
            if c == '\n' {
                return true;
            }
            if c != ' ' && c != '\t' && c != '\r' {
                return false;
            }
            p += 1;
        }
        // End of file with only whitespace
        p == self.pos || (p > self.pos && self.chars[self.pos..p].iter().all(|c| c.is_whitespace()))
    }

    fn is_comment_line(&self) -> bool {
        let mut p = self.pos;
        while p < self.chars.len() && (self.chars[p] == ' ' || self.chars[p] == '\t') {
            p += 1;
        }
        p + 1 < self.chars.len() && self.chars[p] == '-' && self.chars[p + 1] == '-'
    }

    fn current_indent(&self) -> usize {
        let mut indent = 0;
        let mut p = self.pos;
        while p < self.chars.len() {
            match self.chars[p] {
                ' ' => {
                    indent += 1;
                    p += 1;
                }
                '\t' => {
                    indent += 2;
                    p += 1;
                }
                _ => break,
            }
        }
        indent
    }

    fn at_eol(&self) -> bool {
        matches!(self.peek_char(), None | Some('\n') | Some('\r'))
    }

    fn at_eol_after(&self, offset: usize) -> bool {
        let p = self.pos + offset;
        if p >= self.chars.len() {
            return true;
        }
        matches!(self.chars[p], '\n' | '\r')
    }

    fn peek_word(&self) -> String {
        let mut p = self.pos;
        // Skip leading whitespace
        while p < self.chars.len() && (self.chars[p] == ' ' || self.chars[p] == '\t') {
            p += 1;
        }
        let start = p;
        while p < self.chars.len() && (self.chars[p].is_ascii_alphanumeric() || self.chars[p] == '_') {
            p += 1;
        }
        // Check for comment
        if p + 1 <= self.chars.len()
            && start < self.chars.len()
            && self.chars[start] == '-'
            && start + 1 < self.chars.len()
            && self.chars[start + 1] == '-'
        {
            return "--".to_string();
        }
        self.chars[start..p].iter().collect()
    }

    fn peek_word_at(&self, _skip_spaces: usize) -> String {
        let mut p = self.pos;
        // Skip the expected spaces
        while p < self.chars.len() && (self.chars[p] == ' ' || self.chars[p] == '\t') {
            p += 1;
        }
        let start = p;
        while p < self.chars.len() && (self.chars[p].is_ascii_alphanumeric() || self.chars[p] == '_') {
            p += 1;
        }
        self.chars[start..p].iter().collect()
    }

    fn read_word(&mut self) -> String {
        let start = self.pos;
        while let Some(c) = self.peek_char() {
            if c.is_ascii_alphanumeric() || c == '_' {
                self.advance();
            } else {
                break;
            }
        }
        self.chars[start..self.pos].iter().collect()
    }

    fn expect_word(&mut self, expected: &str) -> Result<()> {
        let word = self.read_word();
        if word != expected {
            Err(self.error_msg(format!("expected '{expected}', found '{word}'")))
        } else {
            Ok(())
        }
    }

    fn expect_char(&mut self, expected: char) -> Result<()> {
        match self.peek_char() {
            Some(c) if c == expected => {
                self.advance();
                Ok(())
            }
            Some(c) => Err(self.error_msg(format!("expected '{expected}', found '{c}'"))),
            None => Err(self.error_msg(format!("expected '{expected}', found end of input"))),
        }
    }

    fn try_consume(&mut self, s: &str) -> bool {
        let chars: Vec<char> = s.chars().collect();
        if self.pos + chars.len() > self.chars.len() {
            return false;
        }
        for (i, c) in chars.iter().enumerate() {
            if self.chars[self.pos + i] != *c {
                return false;
            }
        }
        for _ in 0..chars.len() {
            self.advance();
        }
        true
    }

    fn error(&self, message: &str) -> ParseError {
        self.error_msg(message.to_string())
    }

    fn error_msg(&self, message: String) -> ParseError {
        let source_line = if self.line <= self.lines.len() {
            self.lines[self.line - 1].to_string()
        } else {
            String::new()
        };
        ParseError {
            line: self.line,
            col: self.col,
            message,
            source_line,
        }
    }
}

/// Convenience function to parse a ZML source string.
pub fn parse(source: &str) -> std::result::Result<ZmlApp, ParseError> {
    let mut parser = Parser::new(source);
    parser.parse_app()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty() {
        let app = parse("").unwrap();
        assert!(app.permissions.is_none());
        assert!(app.state_block.is_empty());
        assert!(app.body.is_empty());
    }

    #[test]
    fn parse_single_text() {
        let app = parse("Text \"hello\"").unwrap();
        assert_eq!(app.body.len(), 1);
        match &app.body[0] {
            Node::Element { kind, props, .. } => {
                assert_eq!(kind, "Text");
                assert_eq!(props.len(), 1);
                assert_eq!(props[0].key, "content");
            }
        }
    }

    #[test]
    fn parse_state_block() {
        let src = "state\n  count = 0\n  name = \"hello\"\n\nText \"hi\"";
        let app = parse(src).unwrap();
        assert_eq!(app.state_block.len(), 2);
        assert_eq!(app.state_block[0].0, "count");
        assert_eq!(app.state_block[1].0, "name");
    }

    #[test]
    fn parse_permissions() {
        let src = "permissions\n  network = [\"api.example.com\"]\n  camera = false\n\nText \"hi\"";
        let app = parse(src).unwrap();
        let perms = app.permissions.unwrap();
        assert_eq!(perms.network, vec!["api.example.com"]);
        assert!(!perms.camera);
    }

    #[test]
    fn parse_nested_elements() {
        let src = "VStack gap=12\n  Text \"hello\"\n  Button \"+\" variant=primary";
        let app = parse(src).unwrap();
        match &app.body[0] {
            Node::Element { kind, children, props, .. } => {
                assert_eq!(kind, "VStack");
                assert_eq!(children.len(), 2);
                assert_eq!(props.len(), 1);
                assert_eq!(props[0].key, "gap");
            }
        }
    }

    #[test]
    fn parse_handler() {
        let src = "Button \"+\"\n  on click\n    count = count + 1";
        let app = parse(src).unwrap();
        match &app.body[0] {
            Node::Element { handlers, .. } => {
                assert_eq!(handlers.len(), 1);
                assert_eq!(handlers[0].event, "click");
                assert_eq!(handlers[0].actions.len(), 1);
            }
        }
    }

    #[test]
    fn parse_interpolated_string() {
        let src = "Text \"Count: {count}\"";
        let app = parse(src).unwrap();
        match &app.body[0] {
            Node::Element { props, .. } => {
                match &props[0].value {
                    Expr::Interpolated(parts) => {
                        assert_eq!(parts.len(), 2);
                    }
                    _ => panic!("expected interpolated string"),
                }
            }
        }
    }

    #[test]
    fn parse_arithmetic_expr() {
        let src = "state\n  result = 100 * 1.35\n\nText \"hi\"";
        let app = parse(src).unwrap();
        match &app.state_block[0].1 {
            Expr::BinOp(_, BinOp::Mul, _) => {}
            other => panic!("expected BinOp Mul, got: {other:?}"),
        }
    }

    #[test]
    fn parse_comments() {
        let src = "-- this is a comment\nText \"hello\"\n-- another comment";
        let app = parse(src).unwrap();
        assert_eq!(app.body.len(), 1);
    }

    #[test]
    fn error_message_includes_location() {
        let err = parse("Bogus \"hi\"").unwrap_err();
        assert!(err.line > 0);
        assert!(err.message.contains("Bogus"));
    }

    #[test]
    fn parse_full_counter() {
        let src = r#"state
  count = 0

VStack gap=12
  Text "Count: {count}" style=heading
  HStack gap=8
    Button "+" variant=primary
      on click
        count = count + 1
    Button "-"
      on click
        count = count - 1
"#;
        let app = parse(src).unwrap();
        assert_eq!(app.state_block.len(), 1);
        assert_eq!(app.body.len(), 1);
        match &app.body[0] {
            Node::Element { kind, children, .. } => {
                assert_eq!(kind, "VStack");
                assert_eq!(children.len(), 2); // Text, HStack
                match &children[1] {
                    Node::Element { kind, children, .. } => {
                        assert_eq!(kind, "HStack");
                        assert_eq!(children.len(), 2); // two buttons
                    }
                }
            }
        }
    }

    #[test]
    fn parse_bind_prop() {
        let src = "Input bind=amount placeholder=\"Amount\"";
        let app = parse(src).unwrap();
        match &app.body[0] {
            Node::Element { kind, props, .. } => {
                assert_eq!(kind, "Input");
                assert!(props.iter().any(|p| p.key == "bind"));
                assert!(props.iter().any(|p| p.key == "placeholder"));
            }
        }
    }

    #[test]
    fn parse_visible_prop() {
        let src = "Text \"hidden\" visible=show_result";
        let app = parse(src).unwrap();
        match &app.body[0] {
            Node::Element { props, .. } => {
                assert!(props.iter().any(|p| p.key == "visible"));
            }
        }
    }
}
