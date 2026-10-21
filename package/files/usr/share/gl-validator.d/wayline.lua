return {
    set_tunnel_enabled = {
        expected_revision = '^%x+$',
        section_id = '^[%w_-]+$',
        enabled = function(value)
            return type(value) == 'boolean'
        end,
    },
    set_config = {
        expected_revision = '^%x+$',
        config = function(value)
            return type(value) == 'table'
        end,
    },
    get_history = { tunnel = '^[%w_-]+$', target = '^%d+%.%d+%.%d+%.%d+$' },
}
