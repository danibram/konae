/// Same rule as `src/lib/query-bound.ts`. A LIMIT counts only outside
/// parentheses, strings and comments. The editor and an exported Kernel refuse
/// the same query.
pub fn unbounded_query_reason(sql: &str) -> Option<&'static str> {
    let tokens = sql_tokens(sql);
    let top: Vec<&SqlTok> = tokens.iter().filter(|token| token.depth == 0).collect();
    if limit_index(&top).is_some() {
        return None;
    }
    if top.iter().any(|token| token.is_any(&["EXCEPT", "INTERSECT", "UNION"])) {
        return Some("combines more than one query and has no LIMIT on the whole result.");
    }
    if tokens.iter().any(|token| token.is_word("OVER")) {
        return Some("uses OVER, which keeps one result row per stored row, and has no LIMIT.");
    }
    let Some(from) = top.iter().position(|token| token.is_word("FROM")) else {
        return None;
    };
    for pair in top.windows(2) {
        if pair[0].is_word("GROUP") && pair[1].is_word("BY") {
            return Some("groups rows and has no LIMIT, so the result can grow with the data.");
        }
    }
    let Some(select) = top.iter().position(|token| token.is_word("SELECT")) else {
        return Some("reads rows and has no LIMIT.");
    };
    if select >= from || !aggregate_select_list(&tokens.iter().filter(|token| token.start > top[select].start && token.start < top[from].start).collect::<Vec<_>>()) {
        return Some("reads rows and has no LIMIT.");
    }
    None
}

/// Where the LIMIT clause starts among the top-level tokens. `:limit`,
/// `@limit` and `$limit` are named parameters, not the clause, as
/// `limitIndex` in src/lib/query-bound.ts reads them.
pub fn limit_index(top: &[&SqlTok]) -> Option<usize> {
    top.iter().enumerate().position(|(index, token)| {
        let parameter = index > 0 && [":", "@", "$"].iter().any(|symbol| top[index - 1].is_symbol(symbol));
        token.is_word("LIMIT") && !parameter
    })
}

pub fn aggregate_select_list(mut list: &[&SqlTok]) -> bool {
    if list.first().is_some_and(|token| token.is_any(&["ALL", "DISTINCT"])) { list = &list[1..]; }
    !list.is_empty() && split_expressions(list, 0).iter().all(|expression| {
        let mut expression = *expression;
        if let Some(alias) = expression.iter().position(|token| token.depth == 0 && token.is_word("AS")) {
            expression = &expression[..alias];
        } else if expression.last().is_some_and(|token| matches!(token.kind, SqlTokKind::Word | SqlTokKind::Ident))
            && expression.len() > 1 && expression[expression.len() - 2].is_symbol(")") {
            expression = &expression[..expression.len() - 1];
        }
        total_expression(expression)
    })
}

fn split_expressions<'a>(list: &'a [&'a SqlTok], depth: i32) -> Vec<&'a [&'a SqlTok]> {
    let mut parts = Vec::new();
    let mut start = 0;
    for (index, token) in list.iter().enumerate() {
        if token.depth == depth && token.is_symbol(",") { parts.push(&list[start..index]); start = index + 1; }
    }
    parts.push(&list[start..]);
    parts
}

fn total_expression(list: &[&SqlTok]) -> bool {
    if list.len() < 3 { return false; }
    let name = list[0];
    if name.kind != SqlTokKind::Word || !list[1].is_symbol("(") || !list[list.len()-1].is_symbol(")") || list[list.len()-1].depth != name.depth { return false; }
    if list.iter().enumerate().position(|(index, token)| index > 1 && token.is_symbol(")") && token.depth == name.depth) != Some(list.len()-1) { return false; }
    let args = split_expressions(&list[2..list.len()-1], name.depth + 1);
    if name.is_any(&["AVG", "COUNT", "GROUP_CONCAT", "MAX", "MIN", "SUM", "TOTAL"]) {
        return args.iter().all(|arg| !arg.is_empty()) && (args.len() == 1 || name.is_word("GROUP_CONCAT") && args.len() == 2);
    }
    if !name.is_any(&["COALESCE", "IFNULL"]) || args.len() < 2 || name.is_word("IFNULL") && args.len() != 2 { return false; }
    args.iter().any(|arg| total_expression(arg)) && args.iter().all(|arg| total_expression(arg) || constant_expression(arg))
}

fn constant_expression(mut list: &[&SqlTok]) -> bool {
    if list.len() == 2 && (list[0].is_symbol("+") || list[0].is_symbol("-")) { list = &list[1..]; }
    if list.len() != 1 { return false; }
    let token = list[0];
    token.kind == SqlTokKind::String || token.is_word("NULL") || token.kind == SqlTokKind::Symbol && {
        let parts = token.text.split('.').collect::<Vec<_>>();
        parts.len() <= 2 && parts.iter().all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum SqlTokKind { Word, Ident, String, Symbol }

pub struct SqlTok {
    pub kind: SqlTokKind,
    pub text: String,
    pub depth: i32,
    /// Offset in chars into the statement, so a reader can quote the token as written.
    pub start: usize,
}

impl SqlTok {
    pub fn is_word(&self, word: &str) -> bool {
        self.kind == SqlTokKind::Word && self.text.eq_ignore_ascii_case(word)
    }

    pub fn is_any(&self, words: &[&str]) -> bool {
        self.kind == SqlTokKind::Word && words.iter().any(|word| self.text.eq_ignore_ascii_case(word))
    }

    pub fn is_symbol(&self, text: &str) -> bool {
        self.kind == SqlTokKind::Symbol && self.text == text
    }
}

pub fn sql_tokens(sql: &str) -> Vec<SqlTok> {
    let chars: Vec<char> = sql.chars().collect();
    let mut tokens = Vec::new();
    let mut depth = 0i32;
    let mut index = 0;
    while index < chars.len() {
        let char = chars[index];
        let next = chars.get(index + 1).copied();
        if char == '-' && next == Some('-') {
            index += 2;
            while index < chars.len() && chars[index] != '\n' { index += 1; }
            if index < chars.len() { index += 1; }
            continue;
        }
        if char == '/' && next == Some('*') {
            index += 2;
            while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') { index += 1; }
            index = (index + 2).min(chars.len());
            continue;
        }
        if char == '\'' || char == '"' || char == '`' || char == '[' {
            let close = if char == '[' { ']' } else { char };
            let mut end = index + 1;
            let mut text = String::new();
            while end < chars.len() {
                if chars[end] == close {
                    if close != ']' && chars.get(end + 1) == Some(&close) {
                        text.push(close);
                        end += 2;
                        continue;
                    }
                    break;
                }
                text.push(chars[end]);
                end += 1;
            }
            let span_end = (end + 1).min(chars.len());
            tokens.push(if char == '\'' {
                SqlTok { kind: SqlTokKind::String, text: String::new(), depth, start: index }
            } else {
                SqlTok { kind: SqlTokKind::Ident, text, depth, start: index }
            });
            index = span_end;
            continue;
        }
        if is_sql_word_start(char) {
            let mut end = index + 1;
            while end < chars.len() && is_sql_word_cont(chars[end]) { end += 1; }
            tokens.push(SqlTok { kind: SqlTokKind::Word, text: chars[index..end].iter().collect(), depth, start: index });
            index = end;
            continue;
        }
        if char.is_ascii_digit() {
            let mut end = index + 1;
            while end < chars.len() && (chars[end].is_ascii_alphanumeric() || chars[end] == '.' || chars[end] == '_') {
                end += 1;
            }
            tokens.push(SqlTok { kind: SqlTokKind::Symbol, text: chars[index..end].iter().collect(), depth, start: index });
            index = end;
            continue;
        }
        if char.is_whitespace() {
            index += 1;
            continue;
        }
        if char == ')' { depth = (depth - 1).max(0); }
        tokens.push(SqlTok { kind: SqlTokKind::Symbol, text: char.to_string(), depth, start: index });
        if char == '(' { depth += 1; }
        index += 1;
    }
    tokens
}

pub fn is_sql_word_start(char: char) -> bool {
    char.is_ascii_alphabetic() || char == '_' || (char as u32) >= 0x80
}

pub fn is_sql_word_cont(char: char) -> bool {
    char.is_ascii_alphanumeric() || char == '_' || char == '$' || (char as u32) >= 0x80
}
