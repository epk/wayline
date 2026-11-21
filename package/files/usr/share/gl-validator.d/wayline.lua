return {
    purge_history = {
        confirm = function(value)
            return value == true
        end,
    },
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
    get_wifi_history = {
        start = function(value)
            return type(value) == 'number' and value >= 0 and value % 1 == 0
        end,
        ['end'] = function(value)
            return type(value) == 'number' and value >= 0 and value % 1 == 0
        end,
        step = function(value)
            return type(value) == 'number' and value > 0 and value % 1 == 0
        end,
    },
    get_history = {
        tunnel = '^[%w_-]+$',
        target = function(value)
            return type(value) == 'string' and #value <= 253 and value:match('^[%w%.%-]+$') ~= nil
        end,
    },
}
