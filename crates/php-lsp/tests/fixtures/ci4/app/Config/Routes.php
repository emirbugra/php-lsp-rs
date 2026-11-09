<?php
$routes->get('/', 'Home::index', ['as' => 'home']);
$routes->get('users', 'Home::users');
