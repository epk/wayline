return {
    set_config = {
        expected_revision = '^%x+$',
        config = function(value)
            return type(value) == 'table'
        end,
    },
    get_history = { tunnel = '^[%w_-]+$', target = '^%d+%.%d+%.%d+%.%d+$' },
}
