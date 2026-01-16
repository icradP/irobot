fn main() {
    let s = r#"用户输入是“hello”。这是一个问候语。根据规则，当用户进行闲聊、打招呼或问一般性问题时，应优先使用[Conversational]工具。

可用的[Conversational]工具是`chat`。

所以，我应该选择`chat`工具。没有依赖项，因为这是一个独立请求。

输出应该是包含一个步骤的JSON数组：`[{ "tool": "chat", "dependencies": [] }]`。"#;

    let json_slice = match (s.find('['), s.rfind(']')) {
        (Some(i), Some(j)) if j >= i => &s[i..=j],
        _ => s,
    };

    println!("Slice: {}", json_slice);

    // Mock serde parse
    if json_slice.starts_with("[{") {
        println!("Parse might succeed");
    } else {
        println!("Parse likely fails");
    }
}
