<?php
/*
 * Plugin Name: Demo
 */

class Demo_Admin {
    public static function menu() {}
}

function demo_setup() {}

add_action('init', 'demo_setup');
add_action('admin_menu', 'Demo_Admin::menu');
add_filter('the_title', function ($t) { return strtoupper($t); });
add_action('');
