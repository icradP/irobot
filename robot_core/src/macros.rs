/// 处理器自动注册和路由配置的宏
///
/// 使用示例:
/// ```rust,ignore
/// register_handlers!(core => {
///     ConsoleHandler: (ConsoleInput, ConsoleOutput) -> [ConsoleHandler, WebHandler],
///     WebHandler: (WebInput::new(8080).await?, WebOutput::new(8081).await?) -> [WebHandler],
/// });
/// ```

#[macro_export]
macro_rules! register_handlers {
    ($core:expr => {
        $($handler_type:ty: ($input:expr, $output:expr) -> [$($route:ty),+ $(,)?]),+ $(,)?
    }) => {
        {
            $(
                $core.add_input_handler(Box::new($input));
                $core.add_output_handler(
                    $crate::core::router::HandlerId::of::<$handler_type>(),
                    Box::new($output),
                ).await;

                $core.route().add_source_route::<$handler_type>(vec![
                    $($crate::core::router::HandlerId::of::<$route>()),+
                ]);
            )+
        }
    };
}

#[macro_export]
macro_rules! handler_id {
    ($handler:ty) => {
        $crate::core::router::HandlerId::of::<$handler>()
    };
}
